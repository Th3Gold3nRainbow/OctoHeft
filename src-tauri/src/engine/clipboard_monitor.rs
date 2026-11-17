use crate::engine::settings::{SettingsStore, DEFAULT_INTERCEPT_EXTENSIONS};
use crate::shell_integration::ShellIntegration;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

pub static MONITOR_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardUrlDetectedPayload {
    pub url: String,
    pub filename: Option<String>,
    pub is_media: bool,
}

#[derive(Default)]
pub struct ClipboardMonitor {
    active: AtomicBool,
    last_seq: AtomicU32,
    last_copied_text: Arc<Mutex<String>>,
}

#[cfg(target_os = "windows")]
fn current_clipboard_sequence_number() -> u32 {
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(not(target_os = "windows"))]
fn current_clipboard_sequence_number() -> u32 {
    0
}

#[cfg(target_os = "windows")]
fn bring_window_to_foreground_win32(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
        SetForegroundWindow, SetWindowPos, ShowWindow, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOW,
    };

    let Ok(raw_hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = raw_hwnd.0 as windows_sys::Win32::Foundation::HWND;
    if hwnd.is_null() {
        return;
    }

    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }

        let fg_hwnd = GetForegroundWindow();
        let fg_tid = if !fg_hwnd.is_null() {
            GetWindowThreadProcessId(fg_hwnd, std::ptr::null_mut())
        } else {
            0
        };
        let cur_tid = GetCurrentThreadId();
        let attached = fg_tid != 0 && fg_tid != cur_tid && AttachThreadInput(cur_tid, fg_tid, 1) != 0;

        // Topmost pulse nudges DWM z-order above browser windows when triggered from background.
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);

        if attached {
            AttachThreadInput(cur_tid, fg_tid, 0);
        }
    }
}

fn restore_and_focus_main_window(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        #[cfg(target_os = "windows")]
        {
            let win_clone = window.clone();
            let _ = window.run_on_main_thread(move || {
                bring_window_to_foreground_win32(&win_clone);
            });
        }
        let _ = window.set_focus();
    }
}

impl ClipboardMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn set_active(&self, enabled: bool) -> bool {
        self.active.store(enabled, Ordering::Release);
        MONITOR_ACTIVE.store(enabled, Ordering::Release);
        *self.last_copied_text.lock() = String::new();

        if enabled {
            // Snapshot current OS sequence so stale pre-existing clipboard contents do not pop
            // immediately on startup, while clearing last_copied_text ensures re-copying the
            // same URL still fires reliably.
            let seq = current_clipboard_sequence_number();
            self.last_seq.store(seq, Ordering::Release);
            tracing::info!(seq = seq, "Clipboard monitor enabled (500ms Tokio worker)");
        } else {
            tracing::info!("Clipboard monitor disabled");
        }
        enabled
    }

    pub fn process_clipboard_text(
        &self,
        raw_text: Option<&str>,
        intercept_extensions: &str,
    ) -> Option<ClipboardUrlDetectedPayload> {
        let trimmed = raw_text
            .unwrap_or("")
            .trim()
            .trim_start_matches('\u{feff}')
            .trim();

        {
            let mut last = self.last_copied_text.lock();
            if trimmed.is_empty() {
                last.clear();
                return None;
            }
            if *last == trimmed {
                return None;
            }
            *last = trimmed.to_string();
        }

        let matched = classify_clipboard_url(trimmed, intercept_extensions);
        if matched.is_none() {
            tracing::debug!(
                sample_len = trimmed.len(),
                "Clipboard text changed but did not match monitored extensions or media hosts"
            );
        }
        matched
    }

    pub fn start_watcher(
        self: &Arc<Self>,
        settings_store: Arc<SettingsStore>,
        app_handle: AppHandle,
    ) {
        let monitor = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(500));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;
                if !monitor.is_active() {
                    continue;
                }

                let seq = current_clipboard_sequence_number();
                let prev = monitor.last_seq.load(Ordering::Acquire);
                if seq != 0 && seq == prev {
                    continue;
                }

                let read_result =
                    tokio::task::spawn_blocking(ShellIntegration::try_read_clipboard_text).await;
                let maybe_text = match read_result {
                    Ok(Ok(opt)) => {
                        // Only advance last_seq after OpenClipboard succeeded cleanly.
                        monitor.last_seq.store(seq, Ordering::Release);
                        opt
                    }
                    Ok(Err(())) => {
                        tracing::debug!(
                            seq = seq,
                            "Clipboard temporarily locked by another process; retrying on next 500ms tick"
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "Clipboard reader join error");
                        continue;
                    }
                };

                let settings = settings_store.load_or_init();
                let effective_exts = if settings.intercept_extensions.trim().is_empty() {
                    DEFAULT_INTERCEPT_EXTENSIONS
                } else {
                    settings.intercept_extensions.as_str()
                };

                if let Some(payload) =
                    monitor.process_clipboard_text(maybe_text.as_deref(), effective_exts)
                {
                    tracing::info!(
                        url = %payload.url,
                        filename = ?payload.filename,
                        is_media = payload.is_media,
                        "Clipboard monitor matched URL; restoring window and emitting clipboard-url-detected"
                    );

                    restore_and_focus_main_window(&app_handle);
                    let _ = app_handle.emit("clipboard-url-detected", &payload);
                }
            }
        });
    }
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{}", domain))
}

fn has_meaningful_path(parsed: &Url) -> bool {
    parsed
        .path_segments()
        .map(|mut segs| segs.any(|s| !s.is_empty()))
        .unwrap_or(false)
}

fn is_media_domain_or_manifest(parsed: &Url) -> bool {
    let lower_path = parsed.path().to_ascii_lowercase();
    if lower_path.ends_with(".m3u8") || lower_path.ends_with(".mpd") {
        return true;
    }

    let Some(host_raw) = parsed.host_str() else {
        return false;
    };
    let host = host_raw.to_ascii_lowercase();

    if host_matches(&host, "youtu.be") {
        return has_meaningful_path(parsed);
    }

    if host_matches(&host, "youtube.com") || host_matches(&host, "youtube-nocookie.com") {
        let has_watch_v = lower_path == "/watch"
            && parsed
                .query_pairs()
                .any(|(k, v)| k == "v" && !v.trim().is_empty());
        return has_watch_v
            || lower_path.starts_with("/shorts/")
            || lower_path.starts_with("/live/")
            || lower_path.starts_with("/embed/")
            || (lower_path == "/playlist"
                && parsed
                    .query_pairs()
                    .any(|(k, v)| k == "list" && !v.trim().is_empty()));
    }

    if host_matches(&host, "vimeo.com") {
        return has_meaningful_path(parsed);
    }

    if host_matches(&host, "twitch.tv") {
        return lower_path.starts_with("/videos/")
            || lower_path.contains("/clip/")
            || (host == "clips.twitch.tv" && has_meaningful_path(parsed))
            || has_meaningful_path(parsed);
    }

    if host_matches(&host, "dailymotion.com") {
        return lower_path.starts_with("/video/");
    }

    if host_matches(&host, "dai.ly") {
        return has_meaningful_path(parsed);
    }

    if host_matches(&host, "tiktok.com") {
        return lower_path.contains("/video/")
            || host == "vm.tiktok.com"
            || host == "vt.tiktok.com"
            || lower_path.starts_with("/t/");
    }

    if host_matches(&host, "instagram.com") {
        return lower_path.starts_with("/reel/")
            || lower_path.starts_with("/reels/")
            || lower_path.starts_with("/tv/")
            || lower_path.starts_with("/p/");
    }

    if host_matches(&host, "x.com") || host_matches(&host, "twitter.com") {
        return lower_path.contains("/status/");
    }

    if host_matches(&host, "fb.watch") {
        return has_meaningful_path(parsed);
    }

    if host_matches(&host, "facebook.com") {
        return lower_path.starts_with("/watch")
            || lower_path.starts_with("/reel/")
            || lower_path.contains("/videos/");
    }

    if host_matches(&host, "bilibili.com") {
        return lower_path.starts_with("/video/");
    }

    if host_matches(&host, "b23.tv") {
        return has_meaningful_path(parsed);
    }

    if host_matches(&host, "rumble.com") {
        return has_meaningful_path(parsed);
    }

    if host_matches(&host, "soundcloud.com") {
        let non_empty_count = parsed
            .path_segments()
            .map(|segs| segs.filter(|s| !s.is_empty()).count())
            .unwrap_or(0);
        return non_empty_count >= 2;
    }

    false
}

fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn clean_filename_candidate(raw: &str) -> Option<String> {
    let decoded = percent_decode_utf8(raw);
    let clean = decoded
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    if clean.is_empty() || clean == "." || clean == ".." {
        return None;
    }
    Some(clean.to_string())
}

fn extension_from_filename(segment: &str) -> Option<(String, String)> {
    let file = clean_filename_candidate(segment)?;
    let (stem, raw_ext) = file.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let ext = raw_ext.trim().trim_start_matches('.').to_ascii_uppercase();
    if ext.is_empty() || ext.len() > 10 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some((file, ext))
}

fn extract_candidate_filename_and_extension(parsed: &Url) -> Option<(String, String)> {
    // 1. Primary path extraction via url::Url::path() (strips ?query and #fragment automatically)
    let path_only = parsed.path();
    if let Some(last_seg) = path_only.rsplit('/').find(|s| !s.is_empty()) {
        if let Some(pair) = extension_from_filename(last_seg) {
            return Some(pair);
        }
    }

    // 2. Signed download / disposition query fallback (e.g., S3/R2 response-content-disposition)
    for (key, value) in parsed.query_pairs() {
        let key_lower = key.to_ascii_lowercase();
        if matches!(key_lower.as_str(), "filename" | "file" | "name" | "download") {
            if let Some(pair) = extension_from_filename(&value) {
                return Some(pair);
            }
        } else if key_lower == "response-content-disposition" {
            let lower_val = value.to_ascii_lowercase();
            if let Some(idx) = lower_val.find("filename*=") {
                let tail = &value[idx + "filename*=".len()..];
                let fname = tail.split(';').next().unwrap_or(tail);
                let after_charset = fname.rsplit("''").next().unwrap_or(fname);
                if let Some(pair) = extension_from_filename(after_charset) {
                    return Some(pair);
                }
            }
            if let Some(idx) = lower_val.find("filename=") {
                let tail = &value[idx + "filename=".len()..];
                let fname = tail.split(';').next().unwrap_or(tail);
                if let Some(pair) = extension_from_filename(fname) {
                    return Some(pair);
                }
            }
        }
    }

    None
}

fn is_multipart_archive_ext(ext_upper: &str) -> bool {
    // Matches ^R\d{2}$ / ^R[0-9]{2,}$ such as .r00, .r01, .r12, .r99
    ext_upper.len() >= 3
        && ext_upper.starts_with('R')
        && ext_upper[1..].bytes().all(|b| b.is_ascii_digit())
}

fn matches_intercept_extension(ext_upper: &str, intercept_extensions: &str) -> bool {
    let clean_ext = ext_upper.trim().trim_start_matches('.').to_ascii_uppercase();
    if clean_ext.is_empty() {
        return false;
    }

    let mut has_multipart_rule = false;
    for raw_token in intercept_extensions.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
    {
        let mut pat = raw_token
            .trim()
            .trim_start_matches('*')
            .trim_start_matches('.')
            .to_ascii_uppercase();
        pat.retain(|c| c.is_ascii_alphanumeric() || c == '*');
        if pat.is_empty() {
            continue;
        }
        if pat.starts_with("R0") || pat.starts_with("R1") || pat == "R*" || pat == "RAR" {
            has_multipart_rule = true;
        }
        if let Some(prefix) = pat.strip_suffix('*') {
            if !prefix.is_empty() && clean_ext.starts_with(prefix) {
                return true;
            }
        } else if clean_ext == pat {
            return true;
        }
    }

    if has_multipart_rule && is_multipart_archive_ext(&clean_ext) {
        return true;
    }

    false
}

pub fn classify_clipboard_url(
    raw_url: &str,
    intercept_extensions: &str,
) -> Option<ClipboardUrlDetectedPayload> {
    let trimmed = raw_url.trim().trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty()
        || trimmed.contains('\n')
        || trimmed.contains('\r')
        || trimmed.chars().any(char::is_whitespace)
    {
        return None;
    }

    let lower_prefix = trimmed.get(..8).unwrap_or(trimmed).to_ascii_lowercase();
    if !lower_prefix.starts_with("http://") && !lower_prefix.starts_with("https://") {
        return None;
    }

    let parsed = Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }

    let candidate_pair = extract_candidate_filename_and_extension(&parsed);

    if is_media_domain_or_manifest(&parsed) {
        return Some(ClipboardUrlDetectedPayload {
            url: trimmed.to_string(),
            filename: candidate_pair.map(|(f, _)| f),
            is_media: true,
        });
    }

    if let Some((suggested_filename, ext)) = candidate_pair {
        if matches_intercept_extension(&ext, intercept_extensions) {
            return Some(ClipboardUrlDetectedPayload {
                url: trimmed.to_string(),
                filename: Some(suggested_filename),
                is_media: false,
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::settings::DEFAULT_INTERCEPT_EXTENSIONS;

    #[test]
    fn test_classify_archive_binary_and_multipart_urls_with_queries() {
        let rar_match = classify_clipboard_url(
            "https://example.com/test_archive.rar?auth=token123#section",
            DEFAULT_INTERCEPT_EXTENSIONS,
        )
        .expect(".rar with query/hash should match");
        assert!(!rar_match.is_media);
        assert_eq!(rar_match.filename.as_deref(), Some("test_archive.rar"));

        let dat_match = classify_clipboard_url(
            "https://cdn.example.net/payload/weights_v2.dat?download=1",
            DEFAULT_INTERCEPT_EXTENSIONS,
        )
        .expect(".dat should match complete IDM extension list");
        assert!(!dat_match.is_media);
        assert_eq!(dat_match.filename.as_deref(), Some("weights_v2.dat"));

        let exe_match = classify_clipboard_url(
            "https://downloads.example.org/Installer%20Setup.EXE?v=2",
            DEFAULT_INTERCEPT_EXTENSIONS,
        )
        .expect(".EXE uppercase should match case-insensitively");
        assert_eq!(exe_match.filename.as_deref(), Some("Installer Setup.EXE"));

        for part in ["file.r00", "file.r01", "file.r15", "file.r99"] {
            let m = classify_clipboard_url(
                &format!("https://mirror.example.org/bundle/{}?tok=abc", part),
                DEFAULT_INTERCEPT_EXTENSIONS,
            )
            .unwrap_or_else(|| panic!("multipart {} should match", part));
            assert!(!m.is_media);
            assert_eq!(m.filename.as_deref(), Some(part));
        }

        let query_disp = classify_clipboard_url(
            "https://dl.example.org/download?response-content-disposition=attachment%3B%20filename%3D%22setup.iso%22",
            DEFAULT_INTERCEPT_EXTENSIONS,
        )
        .expect("disposition filename ISO should match");
        assert!(!query_disp.is_media);
        assert_eq!(query_disp.filename.as_deref(), Some("setup.iso"));

        assert!(classify_clipboard_url(
            "https://example.com/about-us.html?ref=home",
            DEFAULT_INTERCEPT_EXTENSIONS
        )
        .is_none());
        assert!(classify_clipboard_url(
            "Just a normal copied paragraph referencing file.zip without scheme",
            DEFAULT_INTERCEPT_EXTENSIONS
        )
        .is_none());
        assert!(classify_clipboard_url(
            "ftp://example.com/archive.zip",
            DEFAULT_INTERCEPT_EXTENSIONS
        )
        .is_none());
    }

    #[test]
    fn test_classify_media_domains_and_manifests() {
        let yt = classify_clipboard_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            DEFAULT_INTERCEPT_EXTENSIONS,
        )
        .expect("youtube watch should match");
        assert!(yt.is_media);

        let youtu_be =
            classify_clipboard_url("https://youtu.be/dQw4w9WgXcQ", DEFAULT_INTERCEPT_EXTENSIONS)
                .expect("youtu.be should match");
        assert!(youtu_be.is_media);

        let vimeo =
            classify_clipboard_url("https://vimeo.com/76979871", DEFAULT_INTERCEPT_EXTENSIONS)
                .expect("vimeo video should match");
        assert!(vimeo.is_media);

        let hls = classify_clipboard_url(
            "https://stream.example.com/master.m3u8?auth=1",
            DEFAULT_INTERCEPT_EXTENSIONS,
        )
        .expect("m3u8 manifest should classify as media");
        assert!(hls.is_media);

        assert!(
            classify_clipboard_url("https://www.youtube.com/", DEFAULT_INTERCEPT_EXTENSIONS)
                .is_none()
        );
    }

    #[test]
    fn test_clipboard_monitor_last_copied_text_deduplication_and_recopy() {
        let monitor = ClipboardMonitor::new();
        let first = monitor
            .process_clipboard_text(
                Some("https://example.com/ubuntu.iso?mirror=1"),
                DEFAULT_INTERCEPT_EXTENSIONS,
            )
            .expect("first capture should succeed");
        assert_eq!(first.url, "https://example.com/ubuntu.iso?mirror=1");
        assert_eq!(first.filename.as_deref(), Some("ubuntu.iso"));

        // Identical string on subsequent 500ms tick must NOT re-fire
        assert!(monitor
            .process_clipboard_text(
                Some("https://example.com/ubuntu.iso?mirror=1"),
                DEFAULT_INTERCEPT_EXTENSIONS,
            )
            .is_none());

        // Copying plain text or non-file URL updates last_copied_text cleanly without popup
        assert!(monitor
            .process_clipboard_text(
                Some("https://example.com/page.html"),
                DEFAULT_INTERCEPT_EXTENSIONS,
            )
            .is_none());

        // Re-copying the archive URL afterward triggers again cleanly
        let re_copy = monitor
            .process_clipboard_text(
                Some("https://example.com/ubuntu.iso?mirror=1"),
                DEFAULT_INTERCEPT_EXTENSIONS,
            )
            .expect("re-copied archive URL after other clipboard activity should trigger");
        assert_eq!(re_copy.filename.as_deref(), Some("ubuntu.iso"));
    }
}
