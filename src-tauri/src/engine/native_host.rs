use crate::engine::categories::CategoryDispatcher;
use crate::engine::chunk_worker::{is_googlevideo_url, normalize_request_headers};
use crate::engine::download_engine::DownloadEngine;
use crate::engine::error::EngineError;
use crate::engine::media_muxer::MediaMuxer;
use crate::engine::omniget::{OmnigetCookieEntry, PendingVideoGrab, VideoGrabberPayload};
use crate::engine::settings::SettingsStore;
use crate::engine::state_store::StateStore;
use crate::engine::types::{
    AddDownloadRequest, DownloadCategory, DownloadProgressPayload, DownloadStatus, DownloadTask,
    MediaStreamInfo, SegmentProgress, SegmentStatus,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{error, info, warn};
use url::Url;

pub const NATIVE_PIPE_NAME: &str = r"\\.\pipe\OctoHeftNativeIpc";
const MAX_INCOMING_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTGOING_FRAME_BYTES: usize = 1024 * 1024;

fn default_action() -> String {
    "download".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeMessageRequest {
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, alias = "videoUrl")]
    pub video_url: Option<String>,
    #[serde(default, alias = "audioUrl")]
    pub audio_url: Option<String>,
    #[serde(default, alias = "videoMime")]
    pub video_mime: Option<String>,
    #[serde(default, alias = "audioMime")]
    pub audio_mime: Option<String>,
    #[serde(default, alias = "fileName", alias = "filename")]
    pub file_name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default, alias = "cookieEntries")]
    pub cookie_entries: Option<Vec<OmnigetCookieEntry>>,
    #[serde(default, alias = "referer")]
    pub referrer: Option<String>,
    #[serde(default, alias = "userAgent")]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default, alias = "totalBytes")]
    pub total_bytes: Option<u64>,
    #[serde(default, alias = "startImmediately")]
    pub start_immediately: Option<bool>,
    #[serde(default, alias = "qualityLabel")]
    pub quality_label: Option<String>,
    #[serde(default, alias = "videoCodec")]
    pub video_codec: Option<String>,
    #[serde(default, alias = "audioCodec")]
    pub audio_codec: Option<String>,
    #[serde(default, alias = "audioBitrate")]
    pub audio_bitrate: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct SplitMediaMetadata {
    pub quality_label: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<u32>,
    pub video_mime: Option<String>,
    pub audio_mime: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeMessageResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intercept_extensions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_download_dir: Option<String>,
}

impl NativeMessageResponse {
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            error: Some(msg.into()),
            ..Default::default()
        }
    }
}

/// Returns true when `OctoHeft.exe` is launched explicitly via `--native-host`
/// OR directly spawned by Chromium/Edge/Brave/Firefox Native Messaging.
pub fn should_run_as_native_host(args: &[String]) -> bool {
    args.iter().skip(1).any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "--native-host"
            || lower.starts_with("chrome-extension://")
            || lower.starts_with("moz-extension://")
            || lower.starts_with("--parent-window=")
            || lower.ends_with("com.octoheft.downloader.json")
            || lower.ends_with("com.octoheft.downloader.firefox.json")
    })
}

fn is_valid_external_url(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.to_ascii_lowercase().starts_with("blob:") {
        return Err("Cannot download browser-local blob: URL externally".to_string());
    }
    let parsed = Url::parse(trimmed).map_err(|e| format!("Invalid URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        other => Err(format!("Unsupported URL scheme: {}", other)),
    }
}

fn sanitize_media_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if !is_googlevideo_url(trimmed) {
        return trimmed.to_string();
    }
    let Ok(mut parsed) = Url::parse(trimmed) else {
        return trimmed.to_string();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !matches!(k.as_ref(), "range" | "rn" | "rbuf"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut qp = parsed.query_pairs_mut();
        qp.clear();
        for (k, v) in &kept {
            qp.append_pair(k, v);
        }
    }
    parsed.to_string()
}

/// Reads a single 32-bit LE length-prefixed JSON frame from a Chromium/Firefox Native Messaging stream.
/// Returns `Ok(None)` cleanly upon reaching EOF before the next frame header.
pub async fn read_native_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<NativeMessageRequest>, EngineError> {
    let mut len_buf = [0u8; 4];

    let first = reader.read(&mut len_buf[..1]).await?;
    if first == 0 {
        return Ok(None);
    }

    if let Err(e) = reader.read_exact(&mut len_buf[1..]).await {
        return Err(EngineError::CorruptedStream(format!(
            "Truncated native messaging length header: {}",
            e
        )));
    }

    let msg_len = u32::from_le_bytes(len_buf) as usize;
    if msg_len == 0 || msg_len > MAX_INCOMING_FRAME_BYTES {
        return Err(EngineError::CorruptedStream(format!(
            "Invalid native messaging frame length: {} bytes",
            msg_len
        )));
    }

    let mut payload = vec![0u8; msg_len];
    reader.read_exact(&mut payload).await.map_err(|e| {
        EngineError::CorruptedStream(format!(
            "Unexpected EOF while reading {} byte native message: {}",
            msg_len, e
        ))
    })?;

    serde_json::from_slice::<NativeMessageRequest>(&payload)
        .map(Some)
        .map_err(|e| EngineError::StateParse(format!("Invalid native JSON payload: {}", e)))
}

/// Writes a single 32-bit LE length-prefixed JSON frame to stdout/pipe and flushes immediately.
pub async fn write_native_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    resp: &NativeMessageResponse,
) -> Result<(), EngineError> {
    let json_bytes = serde_json::to_vec(resp).map_err(|e| {
        EngineError::StateParse(format!("Failed to serialize native response: {}", e))
    })?;

    if json_bytes.len() > MAX_OUTGOING_FRAME_BYTES {
        return Err(EngineError::CorruptedStream(format!(
            "Outgoing native messaging payload exceeds 1 MB browser limit ({} bytes)",
            json_bytes.len()
        )));
    }

    let len_prefix = (json_bytes.len() as u32).to_le_bytes();
    writer.write_all(&len_prefix).await?;
    writer.write_all(&json_bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Entrypoint when `OctoHeft.exe --native-host` (or browser Native Host launch) runs.
/// Runs a clean headless Tokio runtime reading stdin frames until EOF without touching WebView2/GUI.
pub fn run_native_host_stdio() -> i32 {
    let (_, logs_dir, _, _) = crate::resolve_runtime_paths();
    crate::engine::logging::init_logging(&logs_dir);

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to initialize native host Tokio runtime: {}", e);
            return 1;
        }
    };

    rt.block_on(async {
        let mut stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();

        loop {
            match read_native_frame(&mut stdin).await {
                Ok(None) => {
                    info!("Native messaging stdin closed cleanly (EOF)");
                    break;
                }
                Ok(Some(req)) => {
                    let resp = process_native_request(req).await;
                    if let Err(e) = write_native_frame(&mut stdout, &resp).await {
                        error!("Failed to write native messaging response: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    warn!("Native messaging frame error: {}", e);
                    let err_resp = NativeMessageResponse::error(e.to_string());
                    let _ = write_native_frame(&mut stdout, &err_resp).await;
                    break;
                }
            }
        }
    });

    0
}

fn spawn_detached_gui(minimized: bool) -> bool {
    #[cfg(all(target_os = "windows", not(test)))]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            CreateProcessW, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, PROCESS_INFORMATION,
            STARTUPINFOW,
        };

        if let Ok(exe_path) = std::env::current_exe() {
            let is_octoheft_bin = exe_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("OctoHeft.exe"))
                .unwrap_or(false);
            if is_octoheft_bin {
                let exe_wide: Vec<u16> = exe_path
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();

                let mut cmd_line = std::ffi::OsString::from("\"");
                cmd_line.push(exe_path.as_os_str());
                if minimized {
                    cmd_line.push("\" --minimized");
                } else {
                    cmd_line.push("\"");
                }
                let mut cmd_wide: Vec<u16> = cmd_line
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();

                // bInheritHandles = 0 prevents the long-lived GUI / WebView2 from holding
                // Chromium's/Python's stdout & stderr Native Messaging pipes open after host exit.
                unsafe {
                    let mut si: STARTUPINFOW = std::mem::zeroed();
                    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
                    let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

                    let created = CreateProcessW(
                        exe_wide.as_ptr(),
                        cmd_wide.as_mut_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        0,
                        DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
                        std::ptr::null(),
                        std::ptr::null(),
                        &si,
                        &mut pi,
                    );

                    if created != 0 {
                        if !pi.hThread.is_null() {
                            CloseHandle(pi.hThread);
                        }
                        if !pi.hProcess.is_null() {
                            CloseHandle(pi.hProcess);
                        }
                        return true;
                    }
                }
            }
        }
    }
    let _ = minimized;
    false
}

pub async fn process_native_request(req: NativeMessageRequest) -> NativeMessageResponse {
    match req.action.to_ascii_lowercase().as_str() {
        "ping" => NativeMessageResponse {
            status: "ok".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            host: Some("OctoHeft".to_string()),
            ..Default::default()
        },
        "get_settings" => {
            if let Ok(resp) = try_forward_over_pipe(&req).await {
                return resp;
            }
            let (storage_dir, _, _, _) = crate::resolve_runtime_paths();
            let app_settings = SettingsStore::new(&storage_dir).load_or_init();
            NativeMessageResponse {
                status: "ok".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
                host: Some("OctoHeft".to_string()),
                delivered: Some("headless-settings".to_string()),
                intercept_extensions: Some(app_settings.intercept_extensions),
                default_download_dir: Some(app_settings.default_download_dir),
                ..Default::default()
            }
        }
        "grab_video" | "open_video_grabber" => {
            let Some(page_url) = req.url.as_deref() else {
                return NativeMessageResponse::error("Missing 'url' field in grab_video request");
            };
            if let Err(err) = is_valid_external_url(page_url) {
                return NativeMessageResponse::error(err);
            }

            if let Ok(resp) = try_forward_over_pipe(&req).await {
                return resp;
            }

            if spawn_detached_gui(false) {
                for _ in 0..16 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    if let Ok(resp) = try_forward_over_pipe(&req).await {
                        return resp;
                    }
                }
            }

            NativeMessageResponse::error(
                "Could not reach OctoMedia UI. Please launch OctoHeft and try again.",
            )
        }
        "download" | "download_media" => {
            let Some(primary_url) = req.video_url.as_deref().or(req.url.as_deref()) else {
                return NativeMessageResponse::error("Missing 'url' field in download request");
            };

            if let Err(err) = is_valid_external_url(primary_url) {
                return NativeMessageResponse::error(err);
            }
            if let Some(ref audio_url) = req.audio_url {
                if let Err(err) = is_valid_external_url(audio_url) {
                    return NativeMessageResponse::error(format!("Invalid audioUrl: {}", err));
                }
            }

            if let Ok(resp) = try_forward_over_pipe(&req).await {
                return resp;
            }

            // If main OctoHeft GUI isn't open yet, launch it silently (with null stdio) and wait briefly for its pipe
            if spawn_detached_gui(true) {
                for _ in 0..8 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    if let Ok(resp) = try_forward_over_pipe(&req).await {
                        return resp;
                    }
                }
            }

            // Fallback when the main GUI daemon is unavailable: queue cleanly to StateStore
            queue_download_offline(&req)
        }
        other => NativeMessageResponse {
            status: "ignored".to_string(),
            reason: Some(format!("Unsupported action: {}", other)),
            ..Default::default()
        },
    }
}

fn insert_header_case_insensitive(
    map: &mut HashMap<String, String>,
    key: &str,
    value: &str,
) {
    let trimmed_key = key.trim();
    let trimmed_val = value.trim();
    if trimmed_key.is_empty() || trimmed_val.is_empty() {
        return;
    }
    let lower = trimmed_key.to_ascii_lowercase();
    let canonical_key: &str = match lower.as_str() {
        "cookie" => "Cookie",
        "referer" => "Referer",
        "user-agent" => "User-Agent",
        "origin" => "Origin",
        "authorization" => "Authorization",
        "range" => "Range",
        _ => trimmed_key,
    };
    if let Some(existing_key) = map
        .keys()
        .find(|k| k.eq_ignore_ascii_case(canonical_key))
        .cloned()
    {
        if existing_key != canonical_key {
            map.remove(&existing_key);
        }
    }
    map.insert(canonical_key.to_string(), trimmed_val.to_string());
}

fn build_headers_from_request(
    req: &NativeMessageRequest,
    target_url: &str,
) -> Option<HashMap<String, String>> {
    let mut map = HashMap::new();
    if let Some(ref extra_headers) = req.headers {
        for (k, v) in extra_headers {
            insert_header_case_insensitive(&mut map, k, v);
        }
    }
    if let Some(ref cookie) = req.cookies {
        insert_header_case_insensitive(&mut map, "Cookie", cookie);
    }
    if let Some(ref referer) = req.referrer {
        insert_header_case_insensitive(&mut map, "Referer", referer);
    }
    if let Some(ref ua) = req.user_agent {
        insert_header_case_insensitive(&mut map, "User-Agent", ua);
    }

    let had_any = !map.is_empty();
    let is_gvid = is_googlevideo_url(target_url);
    if !had_any && !is_gvid {
        return None;
    }

    let normalized: HashMap<String, String> =
        normalize_request_headers(target_url, Some(&map))
            .into_iter()
            .collect();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn sanitize_filename(raw: Option<&str>) -> Option<String> {
    let seg = raw?.rsplit(&['/', '\\'][..]).next()?.trim();

    if seg.is_empty() || seg == "." || seg == ".." {
        return None;
    }

    let cleaned: String = seg
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect();

    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn derive_filename(url_str: &str, requested: Option<&str>, ensure_mp4: bool) -> String {
    let mut name = if let Some(clean) = sanitize_filename(requested) {
        clean
    } else if let Ok(parsed) = Url::parse(url_str) {
        if let Some(last) = parsed.path_segments().and_then(|mut s| s.next_back()) {
            let no_query = last.split('?').next().unwrap_or(last);
            sanitize_filename(Some(no_query)).unwrap_or_else(|| "download.bin".to_string())
        } else {
            "download.bin".to_string()
        }
    } else {
        "download.bin".to_string()
    };

    if ensure_mp4 && !name.to_ascii_lowercase().ends_with(".mp4") {
        if let Some(dot) = name.rfind('.') {
            name.truncate(dot);
        }
        name.push_str(".mp4");
    }

    name
}

fn url_query_param(url_str: &str, key: &str) -> Option<String> {
    let parsed = Url::parse(url_str).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.into_owned())
}

fn infer_video_kind_hint(
    url_str: &str,
    video_mime: Option<&str>,
    video_codec: Option<&str>,
) -> &'static str {
    if let Some(c) = video_codec {
        let lc = c.to_ascii_lowercase();
        if lc.contains("avc") || lc.contains("h.264") || lc.contains("h264") {
            return "avc";
        }
        if lc.contains("vp9") || lc.contains("vp09") || lc.contains("vp8") {
            return "vp9";
        }
        if lc.contains("av1") || lc.contains("av01") {
            return "av1";
        }
        if lc.contains("hevc")
            || lc.contains("h.265")
            || lc.contains("h265")
            || lc.contains("hvc")
            || lc.contains("hev")
        {
            return "hevc";
        }
    }
    if let Some(m) = video_mime {
        let lc = m.to_ascii_lowercase();
        if lc.contains("avc1") || lc.contains("avc3") {
            return "avc";
        }
        if lc.contains("vp9") || lc.contains("vp09") || lc.contains("vp8") {
            return "vp9";
        }
        if lc.contains("av01") || lc.contains("av1") {
            return "av1";
        }
        if lc.contains("hvc1") || lc.contains("hev1") {
            return "hevc";
        }
    }
    if let Some(itag_str) = url_query_param(url_str, "itag") {
        if let Ok(itag) = itag_str.parse::<u32>() {
            match itag {
                133..=138 | 160 | 212 | 264 | 266 | 298 | 299 | 18 | 22 | 37 | 38 => return "avc",
                242..=248
                | 271
                | 272
                | 278
                | 302
                | 303
                | 308
                | 313
                | 315
                | 330..=337
                | 605
                | 606
                | 609
                | 614
                | 616
                | 620
                | 625 => return "vp9",
                394..=401 | 571 | 694..=702 => return "av1",
                _ => {}
            }
        }
    }
    if let Some(m) = video_mime {
        let lc = m.to_ascii_lowercase();
        if lc.contains("webm") {
            return "vp9";
        }
        if lc.contains("mp4") {
            return "avc";
        }
    }
    if let Some(qmime) = url_query_param(url_str, "mime") {
        let lc = qmime.to_ascii_lowercase();
        if lc.contains("webm") {
            return "vp9";
        }
        if lc.contains("mp4") {
            return "avc";
        }
    }
    let url_lc = url_str.to_ascii_lowercase();
    if url_lc.contains(".webm") {
        return "vp9";
    }
    if url_lc.contains(".mkv") {
        return "hevc";
    }
    "avc"
}

fn infer_audio_kind_hint(
    audio_url: Option<&str>,
    audio_mime: Option<&str>,
    audio_codec: Option<&str>,
) -> Option<&'static str> {
    let url_str = audio_url?;
    if let Some(c) = audio_codec {
        let lc = c.to_ascii_lowercase();
        if lc.contains("opus") {
            return Some("opus");
        }
        if lc.contains("vorbis") {
            return Some("vorbis");
        }
        if lc.contains("aac") || lc.contains("mp4a") {
            return Some("aac");
        }
    }
    if let Some(m) = audio_mime {
        let lc = m.to_ascii_lowercase();
        if lc.contains("opus") {
            return Some("opus");
        }
        if lc.contains("vorbis") {
            return Some("vorbis");
        }
        if lc.contains("mp4a") || lc.contains("aac") || lc.contains("mp4") {
            return Some("aac");
        }
        if lc.contains("webm") {
            return Some("opus");
        }
    }
    if let Some(itag_str) = url_query_param(url_str, "itag") {
        if let Ok(itag) = itag_str.parse::<u32>() {
            match itag {
                249..=251 | 338 | 774 => return Some("opus"),
                171 => return Some("vorbis"),
                139..=141 | 256 | 258 | 325 | 327 | 328 => return Some("aac"),
                _ => {}
            }
        }
    }
    if let Some(qmime) = url_query_param(url_str, "mime") {
        let lc = qmime.to_ascii_lowercase();
        if lc.contains("webm") {
            return Some("opus");
        }
        if lc.contains("mp4") {
            return Some("aac");
        }
    }
    Some("aac")
}

fn infer_split_container_ext(
    video_url: &str,
    audio_url: Option<&str>,
    video_mime: Option<&str>,
    audio_mime: Option<&str>,
    video_codec: Option<&str>,
    audio_codec: Option<&str>,
) -> &'static str {
    let v_kind = infer_video_kind_hint(video_url, video_mime, video_codec);
    let a_kind = infer_audio_kind_hint(audio_url, audio_mime, audio_codec);
    match (v_kind, a_kind) {
        ("avc", None | Some("aac")) => "mp4",
        ("vp8" | "vp9" | "av1", None | Some("opus" | "vorbis")) => "webm",
        _ => "mkv",
    }
}

fn replace_or_append_media_ext(mut name: String, target_ext: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let target_suffix = format!(".{}", target_ext);
    if lower.ends_with(&target_suffix) {
        return name;
    }
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot + 1..];
        if matches!(
            ext.to_ascii_lowercase().as_str(),
            "mp4" | "webm" | "mkv" | "mov" | "m4v" | "ts" | "tmp" | "bin"
        ) {
            name.truncate(dot);
        }
    }
    name.push_str(&target_suffix);
    name
}

fn derive_split_media_filename(
    video_url: &str,
    audio_url: Option<&str>,
    requested: Option<&str>,
    video_mime: Option<&str>,
    audio_mime: Option<&str>,
    video_codec: Option<&str>,
    audio_codec: Option<&str>,
) -> String {
    let base = derive_filename(video_url, requested, false);
    let target_ext = infer_split_container_ext(
        video_url,
        audio_url,
        video_mime,
        audio_mime,
        video_codec,
        audio_codec,
    );
    replace_or_append_media_ext(base, target_ext)
}

fn queue_download_offline(req: &NativeMessageRequest) -> NativeMessageResponse {
    let Some(raw_url) = req.video_url.as_deref().or(req.url.as_deref()) else {
        return NativeMessageResponse::error("Missing URL");
    };
    let clean_url = sanitize_media_url(raw_url);

    let (storage_dir, _logs_dir, downloads_dir, _) = crate::resolve_runtime_paths();
    let _ = std::fs::create_dir_all(&storage_dir);

    let app_settings = SettingsStore::new(&storage_dir).load_or_init();
    let dispatcher = CategoryDispatcher::new(&storage_dir, downloads_dir.clone());
    dispatcher.apply_folder_settings(
        PathBuf::from(&app_settings.default_download_dir),
        app_settings.use_category_subfolders,
    );
    let state_store = StateStore::new(&storage_dir);

    let is_split = req.video_url.is_some() || req.audio_url.is_some();
    let requested_name = req.file_name.as_deref().or(req.title.as_deref());
    let file_name = if is_split {
        derive_split_media_filename(
            &clean_url,
            req.audio_url.as_deref(),
            requested_name,
            req.video_mime.as_deref(),
            req.audio_mime.as_deref(),
            req.video_codec.as_deref(),
            req.audio_codec.as_deref(),
        )
    } else {
        derive_filename(&clean_url, requested_name, false)
    };
    let category = dispatcher.classify(&file_name, req.mime.as_deref());
    let save_dir = dispatcher.resolve_destination_dir(&category);
    let _ = std::fs::create_dir_all(&save_dir);
    let save_path = save_dir.join(&file_name);

    let task_id = uuid::Uuid::new_v4().to_string();
    let task = DownloadTask {
        id: task_id.clone(),
        url: clean_url.clone(),
        file_name: file_name.clone(),
        save_path: save_path.to_string_lossy().to_string(),
        total_size: req.total_bytes.unwrap_or(0),
        downloaded_bytes: 0,
        speed: 0,
        eta_seconds: None,
        status: DownloadStatus::Queued,
        category,
        supports_range: true,
        connections: 8,
        segments: Vec::new(),
        error_message: None,
        order_index: Utc::now().timestamp_millis(),
        media_info: None,
        headers: build_headers_from_request(req, &clean_url),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    state_store.add_task(task);
    info!(task_id = %task_id, file = %file_name, "Queued browser capture offline into StateStore");

    NativeMessageResponse {
        status: "success".to_string(),
        task_id: Some(task_id),
        file_name: Some(file_name),
        delivered: Some("offline-queue".to_string()),
        ..Default::default()
    }
}

#[cfg(target_os = "windows")]
async fn try_forward_over_pipe(req: &NativeMessageRequest) -> Result<NativeMessageResponse, String> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let client = {
        let mut attempts = 0;
        loop {
            match ClientOptions::new().open(NATIVE_PIPE_NAME) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(231) && attempts < 5 => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(e) => return Err(format!("Named pipe unavailable: {}", e)),
            }
        }
    };

    let (reader_half, mut writer_half) = tokio::io::split(client);
    let mut line_bytes = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line_bytes.push(b'\n');

    writer_half
        .write_all(&line_bytes)
        .await
        .map_err(|e| format!("Failed writing to OctoHeft IPC pipe: {}", e))?;
    writer_half.flush().await.map_err(|e| e.to_string())?;

    let mut line_reader = BufReader::new(reader_half);
    let mut resp_line = String::new();
    tokio::time::timeout(Duration::from_secs(6), line_reader.read_line(&mut resp_line))
        .await
        .map_err(|_| "Timed out awaiting OctoHeft GUI pipe response".to_string())?
        .map_err(|e| format!("Failed reading IPC pipe response: {}", e))?;

    serde_json::from_str::<NativeMessageResponse>(resp_line.trim())
        .map_err(|e| format!("Invalid IPC pipe JSON response: {}", e))
}

#[cfg(not(target_os = "windows"))]
async fn try_forward_over_pipe(
    _req: &NativeMessageRequest,
) -> Result<NativeMessageResponse, String> {
    Err("Named pipe IPC only available on Windows".to_string())
}

/// Starts the in-process Windows Named Pipe IPC listener inside the running GUI daemon
/// so `--native-host` instances spawned by Chrome/Edge/Brave/Firefox can trigger live downloads immediately.
pub fn start_native_ipc_server(
    engine: Arc<DownloadEngine>,
    settings_store: Arc<SettingsStore>,
    pending_grab: Arc<PendingVideoGrab>,
    app_handle: AppHandle,
) {
    #[cfg(target_os = "windows")]
    {
        use tokio::net::windows::named_pipe::ServerOptions;

        tauri::async_runtime::spawn(async move {
            let mut server = match ServerOptions::new()
                .first_pipe_instance(true)
                .create(NATIVE_PIPE_NAME)
            {
                Ok(s) => s,
                Err(e) => {
                    warn!("Could not bind native IPC named pipe {}: {}", NATIVE_PIPE_NAME, e);
                    return;
                }
            };

            info!("Native Messaging Named Pipe server listening on {}", NATIVE_PIPE_NAME);

            loop {
                if let Err(e) = server.connect().await {
                    warn!("Named pipe connect error: {}", e);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                let connected = server;
                server = match ServerOptions::new().create(NATIVE_PIPE_NAME) {
                    Ok(next) => next,
                    Err(e) => {
                        error!("Failed creating next named pipe instance: {}", e);
                        break;
                    }
                };

                let eng = Arc::clone(&engine);
                let s_store = Arc::clone(&settings_store);
                let p_grab = Arc::clone(&pending_grab);
                let app = app_handle.clone();

                tauri::async_runtime::spawn(async move {
                    let (r_half, mut w_half) = tokio::io::split(connected);
                    let mut line_reader = BufReader::new(r_half);
                    let mut req_line = String::new();

                    if line_reader.read_line(&mut req_line).await.is_ok() && !req_line.trim().is_empty()
                    {
                        let resp = match serde_json::from_str::<NativeMessageRequest>(req_line.trim()) {
                            Ok(req) => handle_live_pipe_request(req, eng, s_store, p_grab, app).await,
                            Err(e) => NativeMessageResponse::error(format!("Invalid JSON: {}", e)),
                        };

                        if let Ok(mut bytes) = serde_json::to_vec(&resp) {
                            bytes.push(b'\n');
                            let _ = w_half.write_all(&bytes).await;
                            let _ = w_half.flush().await;
                        }
                    }
                });
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (engine, settings_store, pending_grab, app_handle);
}

pub async fn dispatch_split_media_task(
    video_url: String,
    audio_url: Option<String>,
    requested_filename: Option<String>,
    save_dir_override: Option<std::path::PathBuf>,
    headers: Option<HashMap<String, String>>,
    expected_bytes: Option<u64>,
    meta: Option<SplitMediaMetadata>,
    engine: Arc<DownloadEngine>,
    app: Option<AppHandle>,
) -> DownloadTask {
    let meta_ref = meta.unwrap_or_default();
    let file_name = derive_split_media_filename(
        &video_url,
        audio_url.as_deref(),
        requested_filename.as_deref(),
        meta_ref.video_mime.as_deref(),
        meta_ref.audio_mime.as_deref(),
        meta_ref.video_codec.as_deref(),
        meta_ref.audio_codec.as_deref(),
    );
    let target_dir = save_dir_override
        .unwrap_or_else(|| engine.category_dispatcher.resolve_destination_dir(&DownloadCategory::Video));
    let _ = std::fs::create_dir_all(&target_dir);
    let dest_file = target_dir.join(&file_name);

    let est_size = expected_bytes.unwrap_or(0);
    let task_id = uuid::Uuid::new_v4().to_string();
    let default_v_codec = infer_video_kind_hint(&video_url, meta_ref.video_mime.as_deref(), None);
    let default_a_codec =
        infer_audio_kind_hint(audio_url.as_deref(), meta_ref.audio_mime.as_deref(), None);
    let media_info = MediaStreamInfo {
        protocol: "YT-MSE".to_string(),
        video_resolution: meta_ref.quality_label,
        video_codec: meta_ref
            .video_codec
            .or_else(|| Some(default_v_codec.to_string())),
        audio_codec: audio_url
            .as_ref()
            .and_then(|_| meta_ref.audio_codec.or_else(|| default_a_codec.map(str::to_string))),
        audio_bitrate: meta_ref.audio_bitrate,
        total_segments: if audio_url.is_some() { 2 } else { 1 },
        completed_segments: 0,
        mux_phase: "downloading".to_string(),
        mux_percent: 0.0,
        ..Default::default()
    };

    let task = DownloadTask {
        id: task_id.clone(),
        url: video_url.clone(),
        file_name: file_name.clone(),
        save_path: dest_file.to_string_lossy().to_string(),
        total_size: est_size,
        downloaded_bytes: 0,
        speed: 0,
        eta_seconds: None,
        status: DownloadStatus::Downloading,
        category: DownloadCategory::Video,
        supports_range: true,
        connections: if audio_url.is_some() { 6 } else { 4 },
        segments: vec![SegmentProgress {
            id: 0,
            start_byte: 0,
            end_byte: est_size.saturating_sub(1),
            current_byte: 0,
            downloaded: 0,
            speed: 0,
            status: SegmentStatus::Downloading,
        }],
        error_message: None,
        order_index: Utc::now().timestamp_millis(),
        media_info: Some(media_info.clone()),
        headers: headers.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    engine.state_store.add_task(task.clone());
    if let Some(ref a) = app {
        let _ = a.emit("tasks-updated", &task);
    }

    let tid = task_id.clone();
    let client = engine.client().clone();
    let store = engine.state_store.clone();
    let app_clone = app.clone();

    tauri::async_runtime::spawn(async move {
        let tid_c = tid.clone();
        let app_c = app_clone.clone();
        let store_c = store.clone();
        let mut current_info = media_info;
        let last_emit = Arc::new(parking_lot::Mutex::new(
            std::time::Instant::now() - Duration::from_secs(1),
        ));

        let res = MediaMuxer::download_and_remux_split_streams(
            &client,
            &video_url,
            audio_url.as_deref(),
            headers.as_ref(),
            &dest_file,
            move |prog| {
                current_info.mux_phase = prog.phase.clone();
                current_info.completed_segments = prog.segments_done;
                current_info.total_segments = prog.segments_total;
                current_info.mux_percent = prog.percent;

                let is_state_edge = prog.phase == "muxing"
                    || prog.phase == "completed"
                    || !prog.active_segments.is_empty();
                let should_emit = prog.phase == "muxing" || prog.phase == "completed" || {
                    let mut guard = last_emit.lock();
                    if guard.elapsed() >= Duration::from_millis(160) || (is_state_edge && prog.percent < 5.0) {
                        *guard = std::time::Instant::now();
                        true
                    } else {
                        false
                    }
                };

                if should_emit {
                    let real_total = if prog.total_bytes > 0 {
                        prog.total_bytes
                    } else {
                        est_size
                    };
                    let real_dl = prog.downloaded_bytes;
                    let seg_count = prog.active_segments.len();

                    if real_total > 0 || seg_count > 0 {
                        store_c.update_total_and_connections(&tid_c, real_total, seg_count);
                    }
                    store_c.update_progress(
                        &tid_c,
                        real_dl,
                        prog.speed_bytes_per_sec,
                        prog.eta_seconds,
                        prog.active_segments.clone(),
                    );
                    store_c.update_task_media_info(&tid_c, current_info.clone());

                    if let Some(ref a) = app_c {
                        let payload = DownloadProgressPayload {
                            id: tid_c.clone(),
                            status: if prog.phase == "completed" {
                                DownloadStatus::Completed
                            } else {
                                DownloadStatus::Downloading
                            },
                            downloaded_bytes: real_dl,
                            total_size: real_total,
                            speed: prog.speed_bytes_per_sec,
                            eta_seconds: prog.eta_seconds,
                            segments: prog.active_segments,
                            error_message: None,
                            media_info: Some(current_info.clone()),
                        };
                        let _ = a.emit("download-progress", &payload);
                    }
                }
            },
        )
        .await;

        match res {
            Ok((final_path, final_len)) => {
                if let Some(mut updated_task) = store.get_task(&tid) {
                    if let Some(fname) = final_path.file_name().and_then(|n| n.to_str()) {
                        updated_task.file_name = fname.to_string();
                    }
                    updated_task.total_size = final_len;
                    updated_task.downloaded_bytes = final_len;
                    updated_task.speed = 0;
                    updated_task.eta_seconds = Some(0);
                    updated_task.save_path = final_path.to_string_lossy().to_string();
                    updated_task.status = DownloadStatus::Completed;
                    updated_task.error_message = None;
                    if let Some(ref mut mi) = updated_task.media_info {
                        mi.mux_phase = "completed".to_string();
                        mi.mux_percent = 100.0;
                    }
                    updated_task.updated_at = Utc::now();
                    store.add_task(updated_task.clone());
                    if let Some(ref a) = app_clone {
                        let _ = a.emit("tasks-updated", &updated_task);
                    }
                }
                store.update_task_status(&tid, DownloadStatus::Completed, None);
                if let Some(ref a) = app_clone {
                    let payload = DownloadProgressPayload {
                        id: tid.clone(),
                        status: DownloadStatus::Completed,
                        downloaded_bytes: final_len,
                        total_size: final_len,
                        speed: 0,
                        eta_seconds: Some(0),
                        segments: vec![],
                        error_message: None,
                        media_info: store.get_task(&tid).and_then(|t| t.media_info),
                    };
                    let _ = a.emit("download-progress", &payload);
                }
            }
            Err(e) => {
                error!(task_id = %tid, error = %e, "Split A/V stream download/mux failed");
                store.update_task_status(&tid, DownloadStatus::Error, Some(e.clone()));
                if let Some(ref a) = app_clone {
                    let payload = DownloadProgressPayload {
                        id: tid.clone(),
                        status: DownloadStatus::Error,
                        downloaded_bytes: 0,
                        total_size: est_size,
                        speed: 0,
                        eta_seconds: None,
                        segments: vec![],
                        error_message: Some(e),
                        media_info: None,
                    };
                    let _ = a.emit("download-progress", &payload);
                }
            }
        }
    });

    task
}

async fn handle_live_pipe_request(
    req: NativeMessageRequest,
    engine: Arc<DownloadEngine>,
    settings_store: Arc<SettingsStore>,
    pending_grab: Arc<PendingVideoGrab>,
    app: AppHandle,
) -> NativeMessageResponse {
    if req.action.eq_ignore_ascii_case("ping") {
        return NativeMessageResponse {
            status: "ok".to_string(),
            delivered: Some("live-ipc".to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            host: Some("OctoHeft".to_string()),
            ..Default::default()
        };
    }

    if req.action.eq_ignore_ascii_case("get_settings") {
        let s = settings_store.load_or_init();
        return NativeMessageResponse {
            status: "ok".to_string(),
            delivered: Some("live-ipc-settings".to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            host: Some("OctoHeft".to_string()),
            intercept_extensions: Some(s.intercept_extensions),
            default_download_dir: Some(s.default_download_dir),
            ..Default::default()
        };
    }

    let action_lc = req.action.to_ascii_lowercase();
    if action_lc == "grab_video" || action_lc == "open_video_grabber" {
        let Some(page_url) = req.url.as_deref() else {
            return NativeMessageResponse::error("Missing 'url' field in grab_video request");
        };
        if let Err(err) = is_valid_external_url(page_url) {
            return NativeMessageResponse::error(err);
        }

        let payload = VideoGrabberPayload {
            url: page_url.to_string(),
            title: req.title.clone(),
            candidate_src: req.video_url.clone(),
            cookies: req.cookies.clone(),
            cookie_entries: req.cookie_entries.clone(),
            user_agent: req.user_agent.clone(),
            referrer: req.referrer.clone(),
        };

        pending_grab.set(payload.clone());

        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        let _ = app.emit("open-video-grabber", &payload);

        info!(url = %page_url, "Routed browser grab_video request to OctoMedia panel");
        return NativeMessageResponse {
            status: "success".to_string(),
            file_name: req.title.clone(),
            delivered: Some("live-ipc-grab".to_string()),
            ..Default::default()
        };
    }

    if action_lc != "download" && action_lc != "download_media" {
        return NativeMessageResponse {
            status: "ignored".to_string(),
            reason: Some(format!("Unsupported action: {}", req.action)),
            ..Default::default()
        };
    }

    let Some(raw_primary_url) = req.video_url.as_deref().or(req.url.as_deref()) else {
        return NativeMessageResponse::error("Missing download URL");
    };

    if let Err(err) = is_valid_external_url(raw_primary_url) {
        return NativeMessageResponse::error(err);
    }
    if let Some(ref a_url) = req.audio_url {
        if let Err(err) = is_valid_external_url(a_url) {
            return NativeMessageResponse::error(format!("Invalid audioUrl: {}", err));
        }
    }

    let primary_url = sanitize_media_url(raw_primary_url);
    let sanitized_audio_url = req.audio_url.as_deref().map(sanitize_media_url);
    let headers = build_headers_from_request(&req, &primary_url);
    let requested_name = req.file_name.as_deref().or(req.title.as_deref());

    let wake_progress_ui = |app_handle: &AppHandle, task_id: &str| {
        if let Some(w) = app_handle.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        let _ = app_handle.emit("open-progress-dialog", task_id);
    };

    // Split Video+Audio MSE streams (e.g. YouTube) -> download concurrently and mux cleanly in pure Rust
    if req.video_url.is_some() || sanitized_audio_url.is_some() {
        let meta = SplitMediaMetadata {
            quality_label: req.quality_label.clone(),
            video_codec: req.video_codec.clone(),
            audio_codec: req.audio_codec.clone(),
            audio_bitrate: req.audio_bitrate,
            video_mime: req.video_mime.clone(),
            audio_mime: req.audio_mime.clone(),
        };
        let task = dispatch_split_media_task(
            primary_url,
            sanitized_audio_url,
            requested_name.map(str::to_string),
            None,
            headers,
            req.total_bytes,
            Some(meta),
            engine,
            Some(app.clone()),
        )
        .await;

        wake_progress_ui(&app, &task.id);
        info!(
            task_id = %task.id,
            file = %task.file_name,
            "Dispatched split A/V MSE remux download via live IPC"
        );
        return NativeMessageResponse {
            status: "success".to_string(),
            task_id: Some(task.id),
            file_name: Some(task.file_name),
            delivered: Some("live-ipc-mux".to_string()),
            ..Default::default()
        };
    }

    let start_now = req.start_immediately.unwrap_or(true);
    let sanitized_file_name = sanitize_filename(requested_name);
    let add_req = AddDownloadRequest {
        url: primary_url.clone(),
        save_path: None,
        file_name: sanitized_file_name,
        connections: None,
        headers: headers.clone(),
        use_category_subfolders: None,
    };

    match engine
        .add_download(add_req, start_now, Some(app.clone()))
        .await
    {
        Ok(task) => {
            info!(task_id = %task.id, file = %task.file_name, "Dispatched browser download via live IPC");
            let _ = app.emit("tasks-updated", &task);
            if start_now {
                wake_progress_ui(&app, &task.id);
            }
            NativeMessageResponse {
                status: "success".to_string(),
                task_id: Some(task.id),
                file_name: Some(task.file_name),
                delivered: Some("live-ipc".to_string()),
                ..Default::default()
            }
        }
        Err(probe_err) => {
            warn!(url = %primary_url, error = %probe_err, "Probe failed via live IPC; storing queued task");
            let file_name = derive_filename(&primary_url, requested_name, false);
            let category = engine
                .category_dispatcher
                .classify(&file_name, req.mime.as_deref());
            let save_dir = engine.category_dispatcher.resolve_destination_dir(&category);
            let _ = std::fs::create_dir_all(&save_dir);

            let task_id = uuid::Uuid::new_v4().to_string();
            let task = DownloadTask {
                id: task_id.clone(),
                url: primary_url,
                file_name: file_name.clone(),
                save_path: save_dir.join(&file_name).to_string_lossy().to_string(),
                total_size: req.total_bytes.unwrap_or(0),
                downloaded_bytes: 0,
                speed: 0,
                eta_seconds: None,
                status: DownloadStatus::Queued,
                category,
                supports_range: true,
                connections: 8,
                segments: Vec::new(),
                error_message: None,
                order_index: Utc::now().timestamp_millis(),
                media_info: None,
                headers,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };

            engine.state_store.add_task(task.clone());
            let _ = app.emit("tasks-updated", &task);

            NativeMessageResponse {
                status: "success".to_string(),
                task_id: Some(task_id),
                file_name: Some(file_name),
                delivered: Some("live-ipc-queued".to_string()),
                ..Default::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_detection_native_host_and_browser_origins() {
        assert!(!should_run_as_native_host(&[
            "OctoHeft.exe".to_string(),
            "--minimized".to_string()
        ]));
        assert!(should_run_as_native_host(&[
            "OctoHeft.exe".to_string(),
            "--native-host".to_string()
        ]));
        assert!(should_run_as_native_host(&[
            "OctoHeft.exe".to_string(),
            "chrome-extension://kalfiofhhhcdnnkjehidagebfikodflp/".to_string()
        ]));
        assert!(should_run_as_native_host(&[
            "OctoHeft.exe".to_string(),
            r"C:\OctoHeft\resources\com.octoheft.downloader.firefox.json".to_string(),
            "octoheft@octoheft.dev".to_string()
        ]));
    }

    #[tokio::test]
    async fn test_native_messaging_length_prefixed_roundtrip() {
        let (mut client_w, mut host_r) = tokio::io::duplex(4096);
        let (mut host_w, mut client_r) = tokio::io::duplex(4096);

        let payload = br#"{"action":"ping"}"#;
        client_w
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await
            .unwrap();
        client_w.write_all(payload).await.unwrap();
        client_w.shutdown().await.unwrap();

        let req = read_native_frame(&mut host_r)
            .await
            .expect("frame decode failed")
            .expect("expected request frame");
        assert_eq!(req.action, "ping");

        let resp = process_native_request(req).await;
        write_native_frame(&mut host_w, &resp).await.unwrap();
        host_w.shutdown().await.unwrap();

        assert!(read_native_frame(&mut host_r).await.unwrap().is_none());

        let mut len_buf = [0u8; 4];
        client_r.read_exact(&mut len_buf).await.unwrap();
        let resp_len = u32::from_le_bytes(len_buf) as usize;
        let mut raw_resp = vec![0u8; resp_len];
        client_r.read_exact(&mut raw_resp).await.unwrap();

        let parsed: NativeMessageResponse = serde_json::from_slice(&raw_resp).unwrap();
        assert_eq!(parsed.status, "ok");
        assert_eq!(parsed.host.as_deref(), Some("OctoHeft"));
    }

    #[tokio::test]
    async fn test_native_host_rejects_blob_urls() {
        let req: NativeMessageRequest = serde_json::from_slice(
            br#"{"action":"download","url":"blob:https://www.youtube.com/1234-5678","fileName":"video.mp4"}"#,
        )
        .unwrap();
        let resp = process_native_request(req).await;
        assert_eq!(resp.status, "error");
        assert!(resp
            .error
            .as_deref()
            .unwrap_or("")
            .contains("blob:"));
    }

    #[test]
    fn test_deserialize_youtube_split_stream_payload() {
        let raw_json = br#"{
            "action": "download",
            "url": "https://rr1---sn.googlevideo.com/videoplayback?itag=137&mime=video%2Fmp4",
            "videoUrl": "https://rr1---sn.googlevideo.com/videoplayback?itag=137&mime=video%2Fmp4",
            "audioUrl": "https://rr1---sn.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4",
            "videoMime": "video/mp4",
            "audioMime": "audio/mp4",
            "filename": "Big Buck Bunny",
            "cookies": "VISITOR_INFO1_LIVE=abc",
            "referrer": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "userAgent": "Mozilla/5.0"
        }"#;

        let req: NativeMessageRequest = serde_json::from_slice(raw_json).unwrap();
        assert_eq!(
            req.video_url.as_deref(),
            Some("https://rr1---sn.googlevideo.com/videoplayback?itag=137&mime=video%2Fmp4")
        );
        assert_eq!(
            req.audio_url.as_deref(),
            Some("https://rr1---sn.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4")
        );
        assert_eq!(
            derive_filename(req.video_url.as_deref().unwrap(), req.file_name.as_deref(), true),
            "Big Buck Bunny.mp4"
        );
    }

    #[test]
    fn test_deserialize_chrome_download_interceptor_payload() {
        let raw_json = br#"{
            "url": "https://cdn.example.org/files/release_v2.rar?token=abc",
            "filename": "builds/release_v2.rar",
            "cookies": "cf_clearance=123; sid=abc",
            "referrer": "https://example.org/downloads",
            "userAgent": "Mozilla/5.0 Chrome/138"
        }"#;

        let req: NativeMessageRequest =
            serde_json::from_slice(raw_json).expect("Failed to parse Chrome interceptor JSON");
        assert_eq!(req.action, "download");
        assert_eq!(req.file_name.as_deref(), Some("builds/release_v2.rar"));
        assert_eq!(
            derive_filename(req.url.as_deref().unwrap(), req.file_name.as_deref(), false),
            "release_v2.rar"
        );

        let headers = build_headers_from_request(&req, req.url.as_deref().unwrap())
            .expect("Expected extracted HTTP headers");
        assert_eq!(
            headers.get("Cookie").map(String::as_str),
            Some("cf_clearance=123; sid=abc")
        );
        assert_eq!(
            headers.get("Referer").map(String::as_str),
            Some("https://example.org/downloads")
        );
        assert_eq!(
            headers.get("User-Agent").map(String::as_str),
            Some("Mozilla/5.0 Chrome/138")
        );
    }

    #[test]
    fn test_deserialize_canonical_snake_case_payload_with_headers() {
        let raw_json = br#"{
            "action": "download_media",
            "title": "Demo 4K",
            "url": "https://rr3---sn.googlevideo.com/videoplayback?itag=313&mime=video%2Fwebm",
            "video_url": "https://rr3---sn.googlevideo.com/videoplayback?itag=313&mime=video%2Fwebm",
            "audio_url": "https://rr3---sn.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4",
            "video_mime": "video/webm",
            "audio_mime": "audio/mp4",
            "quality_label": "2160p 4K",
            "video_codec": "VP9",
            "audio_codec": "AAC",
            "audio_bitrate": 128,
            "file_name": "Demo 4K - 2160p.mp4",
            "total_bytes": 123000000,
            "headers": {
                "referer": "https://www.youtube.com/watch?v=demo4k",
                "User-Agent": "Mozilla/5.0 Chrome/138",
                "Origin": "https://www.youtube.com"
            },
            "cookies": "SID=demo_sid"
        }"#;

        let req: NativeMessageRequest =
            serde_json::from_slice(raw_json).expect("Canonical snake_case payload must parse cleanly");
        assert_eq!(req.action, "download_media");
        assert_eq!(req.title.as_deref(), Some("Demo 4K"));
        assert_eq!(req.file_name.as_deref(), Some("Demo 4K - 2160p.mp4"));
        assert_eq!(req.quality_label.as_deref(), Some("2160p 4K"));
        assert_eq!(req.audio_bitrate, Some(128));
        assert_eq!(req.total_bytes, Some(123_000_000));

        let headers = build_headers_from_request(&req, req.url.as_deref().unwrap())
            .expect("Expected merged HTTP headers");
        assert_eq!(headers.get("Cookie").map(String::as_str), Some("SID=demo_sid"));
        assert_eq!(
            headers.get("Referer").map(String::as_str),
            Some("https://www.youtube.com/")
        );
        assert!(!headers.contains_key("referer"));
        assert_eq!(
            headers.get("Origin").map(String::as_str),
            Some("https://www.youtube.com")
        );
    }

    #[test]
    fn test_download_media_title_fallback_and_url_sanitization() {
        let raw_json = br#"{
            "action": "download_media",
            "title": "Cool Talk 2160p",
            "video_url": "https://rr5---sn.googlevideo.com/videoplayback?itag=313&range=0-1000&rn=1&rbuf=512&sig=ABC",
            "audio_url": "https://rr5---sn.googlevideo.com/videoplayback?itag=140&pot=POT123"
        }"#;
        let req: NativeMessageRequest = serde_json::from_slice(raw_json).unwrap();
        assert_eq!(req.action, "download_media");
        assert_eq!(req.file_name, None);
        assert_eq!(req.title.as_deref(), Some("Cool Talk 2160p"));

        let cleaned = sanitize_media_url(req.video_url.as_deref().unwrap());
        assert!(!cleaned.contains("range="));
        assert!(!cleaned.contains("rn="));
        assert!(!cleaned.contains("rbuf="));
        assert!(cleaned.contains("sig=ABC"));

        let hdrs = build_headers_from_request(&req, &cleaned).unwrap();
        assert_eq!(hdrs.get("Origin").map(String::as_str), Some("https://www.youtube.com"));
        assert_eq!(hdrs.get("Referer").map(String::as_str), Some("https://www.youtube.com/"));
        assert!(hdrs.contains_key("User-Agent"));
        assert_eq!(
            derive_filename(&cleaned, req.file_name.as_deref().or(req.title.as_deref()), true),
            "Cool Talk 2160p.mp4"
        );
    }

    #[test]
    fn test_sanitize_filename_edge_cases() {
        assert_eq!(
            sanitize_filename(Some(r"..\..\temp\setup:1.exe")),
            Some("setup_1.exe".to_string())
        );
        assert_eq!(sanitize_filename(Some("..")), None);
        assert_eq!(sanitize_filename(Some("   ")), None);
    }

    #[test]
    fn test_derive_split_media_filename_adaptive_containers() {
        // VP9 (itag=313) + Opus (itag=251) -> .webm
        assert_eq!(
            derive_split_media_filename(
                "https://rr1.googlevideo.com/videoplayback?itag=313&mime=video%2Fwebm",
                Some("https://rr1.googlevideo.com/videoplayback?itag=251&mime=audio%2Fwebm"),
                Some("4K Nature"),
                Some("video/webm"),
                Some("audio/webm"),
                Some("VP9"),
                Some("Opus"),
            ),
            "4K Nature.webm"
        );

        // AV1 in MP4 box (itag=401, video/mp4!) + Opus (itag=251) -> .webm
        assert_eq!(
            derive_split_media_filename(
                "https://rr1.googlevideo.com/videoplayback?itag=401&mime=video%2Fmp4",
                Some("https://rr1.googlevideo.com/videoplayback?itag=251&mime=audio%2Fwebm"),
                Some("8K Showcase - 2160p"),
                Some("video/mp4"),
                Some("audio/webm"),
                None,
                None,
            ),
            "8K Showcase - 2160p.webm"
        );

        // VP9 (itag=313) + AAC (itag=140) -> .mkv (even if caller supplied stale .mp4 name)
        assert_eq!(
            derive_split_media_filename(
                "https://rr1.googlevideo.com/videoplayback?itag=313&mime=video%2Fwebm",
                Some("https://rr1.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4"),
                Some("Demo 4K - 2160p.mp4"),
                Some("video/webm"),
                Some("audio/mp4"),
                Some("VP9"),
                Some("AAC"),
            ),
            "Demo 4K - 2160p.mkv"
        );

        // AVC1 (itag=137) + AAC (itag=140) -> .mp4 and preserves dotted version numbers in title
        assert_eq!(
            derive_split_media_filename(
                "https://rr1.googlevideo.com/videoplayback?itag=137&mime=video%2Fmp4",
                Some("https://rr1.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4"),
                Some("Rust Engine v2.1"),
                Some("video/mp4"),
                Some("audio/mp4"),
                Some("H.264 / AVC"),
                Some("AAC"),
            ),
            "Rust Engine v2.1.mp4"
        );
    }

    #[test]
    fn test_deserialize_grab_video_with_cookie_entries() {
        let raw_json = br#"{
            "action": "grab_video",
            "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "title": "Big Buck Bunny 60fps 4K",
            "cookies": "SID=abc; PREF=1",
            "cookie_entries": [
                {
                    "domain": ".youtube.com",
                    "path": "/",
                    "secure": true,
                    "expires": 1900000000,
                    "http_only": true,
                    "name": "SID",
                    "value": "abc"
                }
            ],
            "referrer": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "user_agent": "Mozilla/5.0 Chrome/138"
        }"#;

        let req: NativeMessageRequest =
            serde_json::from_slice(raw_json).expect("Failed to parse grab_video payload");
        assert_eq!(req.action, "grab_video");
        assert_eq!(
            req.url.as_deref(),
            Some("https://www.youtube.com/watch?v=aqz-KE-bpKQ")
        );
        assert_eq!(req.cookie_entries.as_ref().map(|v| v.len()), Some(1));
    }
}
