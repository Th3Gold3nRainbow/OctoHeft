use crate::engine::chunk_worker::DEFAULT_BROWSER_USER_AGENT;
use crate::engine::omniget::cookies::build_temp_cookie_file;
use crate::engine::omniget::process::{build_tokio_command, kill_process_tree, ResolvedTools};
use crate::engine::omniget::{
    OmnigetCookieEntry, OmnigetFetchRequest, OmnigetFormatOption, OmnigetMediaInfo,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tracing::{debug, info, warn};
use url::Url;

const METADATA_TIMEOUT_SECS: u64 = 45;

pub fn validate_http_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL cannot be empty".to_string());
    }
    if trimmed.to_ascii_lowercase().starts_with("blob:") {
        return Err(
            "Browser-local blob: URLs cannot be downloaded directly. Paste the page URL instead."
                .to_string(),
        );
    }
    let parsed = Url::parse(trimmed).map_err(|e| format!("Invalid URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed.to_string()),
        other => Err(format!("Unsupported URL scheme '{}'", other)),
    }
}

pub fn is_youtube_url(raw: &str) -> bool {
    if let Ok(parsed) = Url::parse(raw.trim()) {
        if let Some(host) = parsed.host_str() {
            let h = host.to_ascii_lowercase();
            return h == "youtube.com"
                || h.ends_with(".youtube.com")
                || h == "youtube-nocookie.com"
                || h.ends_with(".youtube-nocookie.com")
                || h == "youtu.be";
        }
    }
    false
}

pub fn format_duration(secs_opt: Option<f64>) -> Option<String> {
    let secs = secs_opt?;
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        Some(format!("{}:{:02}:{:02}", h, m, s))
    } else {
        Some(format!("{}:{:02}", m, s))
    }
}

/// Fetches available video & audio formats cleanly via `yt-dlp --dump-single-json`,
/// with SABR/client cascade for YouTube and direct HTTP fallback for raw CDN media URLs.
pub async fn fetch_media_formats(req: &OmnigetFetchRequest) -> Result<OmnigetMediaInfo, String> {
    let clean_url = validate_http_url(&req.url)?;
    let tools = ResolvedTools::discover();

    let ytdlp_path = match tools.ytdlp_path {
        Some(ref p) => p.clone(),
        None => {
            if let Some(direct) = try_direct_http_probe(req, &clean_url).await {
                return Ok(direct);
            }
            return Err(
                "yt-dlp is not installed yet. Click 'Auto-Setup Tools' in OctoMedia to download yt-dlp and FFmpeg automatically."
                    .to_string(),
            );
        }
    };

    let temp_cookies = build_temp_cookie_file(
        &clean_url,
        req.cookie_entries.as_deref(),
        req.cookies.as_deref(),
    )?;
    let cookie_path = temp_cookies.as_ref().map(|c| c.path().to_path_buf());
    let is_yt = is_youtube_url(&clean_url);

    // Strategy 1: Standard extraction with supplied cookies
    let first = run_ytdlp_dump_json(
        &ytdlp_path,
        &tools,
        &clean_url,
        cookie_path.as_deref(),
        req.user_agent.as_deref(),
        req.referrer.as_deref(),
        None,
    )
    .await;

    let mut chosen_json: Option<Value> = None;
    let mut last_err: Option<String> = None;

    match first {
        Ok(val) => {
            if is_yt && is_youtube_json_degraded(&val) {
                warn!("Initial YouTube metadata only exposed low-res progressive formats; retrying with web_safari/mweb client cascade");
                chosen_json = Some(val);
            } else {
                return parse_ytdlp_json_to_media_info(&val, &clean_url);
            }
        }
        Err(err) => {
            last_err = Some(err);
        }
    }

    // Strategy 2 (YouTube cascade): web_safari + mweb clients (handles SABR / missing po_token cleanly)
    if is_yt {
        let retry_clients = run_ytdlp_dump_json(
            &ytdlp_path,
            &tools,
            &clean_url,
            cookie_path.as_deref(),
            req.user_agent.as_deref(),
            req.referrer.as_deref(),
            Some("youtube:player_client=default,web_safari,mweb"),
        )
        .await;

        match retry_clients {
            Ok(val) => {
                if !is_youtube_json_degraded(&val) || chosen_json.is_none() {
                    chosen_json = Some(val);
                }
            }
            Err(e) => {
                last_err = Some(e);
            }
        }

        // Strategy 3: Cookieless fallback if browser cookies triggered age/account challenge mismatch
        if cookie_path.is_some()
            && chosen_json
                .as_ref()
                .map(is_youtube_json_degraded)
                .unwrap_or(true)
        {
            info!("Retrying YouTube metadata cookieless with web_safari/mweb fallback");
            let cookieless = run_ytdlp_dump_json(
                &ytdlp_path,
                &tools,
                &clean_url,
                None,
                req.user_agent.as_deref(),
                req.referrer.as_deref(),
                Some("youtube:player_client=default,web_safari,mweb"),
            )
            .await;

            if let Ok(val) = cookieless {
                if !is_youtube_json_degraded(&val) || chosen_json.is_none() {
                    chosen_json = Some(val);
                }
            }
        }
    }

    if let Some(json_val) = chosen_json {
        return parse_ytdlp_json_to_media_info(&json_val, &clean_url);
    }

    // Fallback: direct HTTP media probe (e.g., <video src="https://cdn.site/clip.mp4"> on unsupported HTML page)
    if let Some(direct) = try_direct_http_probe(req, &clean_url).await {
        return Ok(direct);
    }

    Err(last_err.unwrap_or_else(|| "Failed to extract video formats".to_string()))
}

async fn run_ytdlp_dump_json(
    ytdlp_path: &Path,
    tools: &ResolvedTools,
    url: &str,
    cookie_path: Option<&Path>,
    user_agent: Option<&str>,
    referrer: Option<&str>,
    extractor_args: Option<&str>,
) -> Result<Value, String> {
    let mut cmd = build_tokio_command(ytdlp_path, tools);
    cmd.args([
        "--dump-single-json",
        "--ignore-config",
        "--no-warnings",
        "--no-playlist",
        "--encoding",
        "utf-8",
        "--skip-download",
    ]);

    let ua = user_agent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BROWSER_USER_AGENT);
    cmd.args(["--user-agent", ua]);

    if let Some(r) = referrer.map(str::trim).filter(|s| !s.is_empty()) {
        cmd.args(["--referer", r]);
    }
    if let Some((kind, path)) = &tools.js_runtime {
        cmd.args(["--js-runtimes", &format!("{}:{}", kind, path.display())]);
    }
    if let Some(cp) = cookie_path {
        cmd.args(["--cookies", &cp.to_string_lossy()]);
    }
    if let Some(ea) = extractor_args {
        cmd.args(["--extractor-args", ea]);
    }

    cmd.arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    debug!(url = %url, extractor_args = ?extractor_args, "Executing yt-dlp metadata extraction");
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed spawning yt-dlp: {}", e))?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let wait_fut = async {
        use tokio::io::AsyncReadExt;
        let mut out_buf = Vec::new();
        let mut err_buf = Vec::new();
        if let Some(mut so) = stdout_pipe {
            let _ = so.read_to_end(&mut out_buf).await;
        }
        if let Some(mut se) = stderr_pipe {
            let _ = se.read_to_end(&mut err_buf).await;
        }
        let status = child.wait().await;
        (status, out_buf, err_buf)
    };

    let (status_res, out_buf, err_buf) =
        match tokio::time::timeout(Duration::from_secs(METADATA_TIMEOUT_SECS), wait_fut).await {
            Ok(tup) => tup,
            Err(_) => {
                kill_process_tree(&mut child).await;
                return Err("Metadata lookup timed out after 45s".to_string());
            }
        };

    let status = status_res.map_err(|e| format!("yt-dlp wait failed: {}", e))?;
    let stdout_str = String::from_utf8_lossy(&out_buf);
    let stderr_str = String::from_utf8_lossy(&err_buf);

    if !status.success() {
        let concise_err = stderr_str
            .lines()
            .rfind(|l| l.contains("ERROR:"))
            .or_else(|| stderr_str.lines().last())
            .unwrap_or("yt-dlp failed to inspect URL")
            .trim()
            .to_string();
        return Err(concise_err);
    }

    serde_json::from_str::<Value>(stdout_str.trim())
        .map_err(|e| format!("Failed parsing yt-dlp JSON metadata: {}", e))
}

fn is_youtube_json_degraded(val: &Value) -> bool {
    let target = first_entry_or_root(val);
    let Some(formats) = target.get("formats").and_then(|f| f.as_array()) else {
        return true;
    };
    let max_h = formats
        .iter()
        .filter(|f| {
            let vc = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
            vc != "none" && !vc.is_empty()
        })
        .filter_map(|f| f.get("height").and_then(|h| h.as_u64()))
        .max()
        .unwrap_or(0);
    max_h <= 360
}

fn first_entry_or_root(val: &Value) -> &Value {
    if let Some(entries) = val.get("entries").and_then(|e| e.as_array()) {
        if let Some(first) = entries.first() {
            return first;
        }
    }
    val
}

#[derive(Debug, Clone)]
struct RawAudioTrack {
    format_id: String,
    acodec_raw: String,
    acodec_label: String,
    ext: String,
    abr: f64,
    size_bytes: Option<u64>,
}

fn estimate_stream_bytes(fmt: &Value, duration_secs: Option<f64>) -> Option<u64> {
    if let Some(fs) = fmt.get("filesize").and_then(|v| v.as_u64()) {
        if fs > 0 {
            return Some(fs);
        }
    }
    if let Some(fs_approx) = fmt.get("filesize_approx").and_then(|v| v.as_u64()) {
        if fs_approx > 0 {
            return Some(fs_approx);
        }
    }
    let dur = duration_secs.unwrap_or(0.0);
    let tbr = fmt
        .get("tbr")
        .and_then(|v| v.as_f64())
        .or_else(|| fmt.get("vbr").and_then(|v| v.as_f64()))
        .or_else(|| fmt.get("abr").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    if dur > 0.0 && tbr > 0.0 {
        Some(((tbr * 1000.0 / 8.0) * dur).round() as u64)
    } else {
        None
    }
}

fn classify_vcodec(vcodec: &str) -> (&'static str, &'static str) {
    let lc = vcodec.to_ascii_lowercase();
    if lc.starts_with("av01") || lc.contains("av1") {
        ("av1", "AV1")
    } else if lc.starts_with("vp09") || lc.starts_with("vp9") {
        ("vp9", "VP9")
    } else if lc.starts_with("vp08") || lc.starts_with("vp8") {
        ("vp8", "VP8")
    } else if lc.starts_with("hev") || lc.starts_with("hvc") || lc.contains("h265") || lc.contains("hevc") {
        ("hevc", "H.265 / HEVC")
    } else if lc.starts_with("avc") || lc.contains("h264") {
        ("avc", "H.264 / AVC")
    } else {
        ("other", "Video")
    }
}

fn classify_acodec(acodec: &str) -> (&'static str, &'static str) {
    let lc = acodec.to_ascii_lowercase();
    if lc == "none" || lc.is_empty() {
        ("none", "None")
    } else if lc.contains("opus") {
        ("opus", "Opus")
    } else if lc.contains("vorbis") {
        ("vorbis", "Vorbis")
    } else if lc.starts_with("mp4a") || lc.contains("aac") {
        ("aac", "AAC")
    } else if lc.contains("mp3") {
        ("mp3", "MP3")
    } else {
        ("other", "Audio")
    }
}

fn resolution_tag(height: u32) -> String {
    match height {
        4320.. => "4320p 8K".to_string(),
        2160..=4319 => "2160p 4K".to_string(),
        1440..=2159 => "1440p 2K".to_string(),
        1080..=1439 => "1080p FHD".to_string(),
        720..=1079 => "720p HD".to_string(),
        h if h > 0 => format!("{}p", h),
        _ => "Best".to_string(),
    }
}

pub fn parse_ytdlp_json_to_media_info(
    val: &Value,
    fallback_url: &str,
) -> Result<OmnigetMediaInfo, String> {
    let root = first_entry_or_root(val);
    let title = root
        .get("title")
        .or_else(|| root.get("fulltitle"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Untitled Video".to_string());

    let uploader = root
        .get("uploader")
        .or_else(|| root.get("channel"))
        .or_else(|| root.get("creator"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let thumbnail = root
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            root.get("thumbnails")
                .and_then(|t| t.as_array())
                .and_then(|arr| arr.last())
                .and_then(|item| item.get("url"))
                .and_then(|u| u.as_str())
                .map(str::to_string)
        });

    let duration_seconds = root.get("duration").and_then(|v| v.as_f64());
    let duration_formatted = format_duration(duration_seconds);
    let extractor = root
        .get("extractor_key")
        .or_else(|| root.get("extractor"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let canonical_url = root
        .get("webpage_url")
        .or_else(|| root.get("original_url"))
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("http"))
        .unwrap_or(fallback_url)
        .to_string();

    let raw_formats = root
        .get("formats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 1. Collect audio-only streams to compute accurate pairings and file size estimates
    let mut audio_tracks: Vec<RawAudioTrack> = Vec::new();
    for fmt in &raw_formats {
        let Some(format_id) = fmt.get("format_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let vcodec = fmt.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
        let acodec = fmt.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
        if (vcodec == "none" || vcodec.is_empty()) && acodec != "none" && !acodec.is_empty() {
            let (akind, alabel) = classify_acodec(acodec);
            let ext = fmt
                .get("ext")
                .and_then(|v| v.as_str())
                .unwrap_or("m4a")
                .to_string();
            let abr = fmt
                .get("abr")
                .and_then(|v| v.as_f64())
                .or_else(|| fmt.get("tbr").and_then(|v| v.as_f64()))
                .unwrap_or(128.0);
            audio_tracks.push(RawAudioTrack {
                format_id: format_id.to_string(),
                acodec_raw: akind.to_string(),
                acodec_label: alabel.to_string(),
                ext,
                abr,
                size_bytes: estimate_stream_bytes(fmt, duration_seconds),
            });
        }
    }

    audio_tracks.sort_by(|a, b| {
        b.abr
            .partial_cmp(&a.abr)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let best_aac = audio_tracks
        .iter()
        .find(|a| a.acodec_raw == "aac" || a.ext == "m4a")
        .cloned();
    let best_opus = audio_tracks
        .iter()
        .find(|a| a.acodec_raw == "opus" || a.ext == "webm")
        .cloned();
    let best_any_audio = audio_tracks.first().cloned();

    // 2. Collect curated video options grouped cleanly by (height, container_tier)
    //    key: (Reverse height, tier: 0=VP9/AV1 crisp, 1=H264 MP4 compatible)
    let mut best_by_bucket: BTreeMap<(std::cmp::Reverse<u32>, u8), (f64, OmnigetFormatOption)> =
        BTreeMap::new();

    for fmt in &raw_formats {
        let Some(format_id) = fmt.get("format_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let proto = fmt
            .get("protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if proto.contains("mhtml") || format_id.starts_with("sb") {
            continue;
        }
        let vcodec = fmt.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
        if vcodec == "none" || vcodec.is_empty() {
            continue;
        }

        let width = fmt.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let raw_h = fmt.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        // Portrait shorts swap dimensions on some sites; use shortest side for resolution tier
        let height = if width > 0 && raw_h > 0 {
            width.min(raw_h)
        } else {
            raw_h
        };
        if height == 0 {
            continue;
        }

        let acodec = fmt.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
        let is_muxed = acodec != "none" && !acodec.is_empty();
        let fps = fmt
            .get("fps")
            .and_then(|v| v.as_f64())
            .map(|f| f.round() as u32)
            .filter(|&f| f > 0);
        let tbr = fmt
            .get("vbr")
            .and_then(|v| v.as_f64())
            .or_else(|| fmt.get("tbr").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        let ext_raw = fmt
            .get("ext")
            .and_then(|v| v.as_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        let dyn_range = fmt
            .get("dynamic_range")
            .and_then(|v| v.as_str())
            .unwrap_or("SDR");
        let is_hdr = !dyn_range.is_empty() && !dyn_range.eq_ignore_ascii_case("SDR");

        let (vkind, vlabel_base) = classify_vcodec(vcodec);
        let vlabel = if is_hdr {
            format!("{} {}", vlabel_base, dyn_range)
        } else {
            vlabel_base.to_string()
        };

        let vid_size = estimate_stream_bytes(fmt, duration_seconds);

        let (container, alabel, combined_size, format_spec, bucket_tier) = if is_muxed {
            let (_ak, al) = classify_acodec(acodec);
            let cont = if ext_raw == "webm" {
                "webm"
            } else if ext_raw == "mkv" {
                "mkv"
            } else if vkind == "avc" {
                "mp4"
            } else {
                "mkv"
            };
            (
                cont.to_string(),
                al.to_string(),
                vid_size,
                format!("{}/best[height<={}]/best", format_id, height),
                if cont == "mp4" { 1u8 } else { 0u8 },
            )
        } else if vkind == "avc" {
            let chosen_aud = best_aac.as_ref().or(best_any_audio.as_ref());
            let cont = match chosen_aud.map(|a| a.acodec_raw.as_str()) {
                Some("aac") | None => "mp4",
                _ => "mkv",
            };
            let aud_spec = chosen_aud
                .map(|a| a.format_id.as_str())
                .unwrap_or("bestaudio[ext=m4a]/bestaudio");
            let combined = match (vid_size, chosen_aud.and_then(|a| a.size_bytes)) {
                (Some(v), Some(a)) => Some(v + a),
                (Some(v), None) => Some(v),
                _ => None,
            };
            (
                cont.to_string(),
                chosen_aud
                    .map(|a| a.acodec_label.clone())
                    .unwrap_or_else(|| "AAC".to_string()),
                combined,
                format!(
                    "{}+{}/bestvideo[height<={}][vcodec^=avc1]+bestaudio[ext=m4a]/bestvideo[height<={}]+bestaudio/best[height<={}]",
                    format_id, aud_spec, height, height, height
                ),
                1u8,
            )
        } else if vkind == "vp9" || vkind == "vp8" {
            let chosen_aud = best_opus.as_ref().or(best_any_audio.as_ref());
            let cont = match chosen_aud.map(|a| a.acodec_raw.as_str()) {
                Some("opus") | Some("vorbis") => "webm",
                _ => "mkv",
            };
            let aud_spec = chosen_aud
                .map(|a| a.format_id.as_str())
                .unwrap_or("bestaudio");
            let combined = match (vid_size, chosen_aud.and_then(|a| a.size_bytes)) {
                (Some(v), Some(a)) => Some(v + a),
                (Some(v), None) => Some(v),
                _ => None,
            };
            (
                cont.to_string(),
                chosen_aud
                    .map(|a| a.acodec_label.clone())
                    .unwrap_or_else(|| "Opus".to_string()),
                combined,
                format!(
                    "{}+{}/bestvideo[height<={}]+bestaudio/best[height<={}]",
                    format_id, aud_spec, height, height
                ),
                0u8,
            )
        } else {
            // AV1 / HEVC / Other adaptive -> safe Matroska (.mkv) lossless muxing
            let chosen_aud = best_opus
                .as_ref()
                .or(best_aac.as_ref())
                .or(best_any_audio.as_ref());
            let aud_spec = chosen_aud
                .map(|a| a.format_id.as_str())
                .unwrap_or("bestaudio");
            let combined = match (vid_size, chosen_aud.and_then(|a| a.size_bytes)) {
                (Some(v), Some(a)) => Some(v + a),
                (Some(v), None) => Some(v),
                _ => None,
            };
            (
                "mkv".to_string(),
                chosen_aud
                    .map(|a| a.acodec_label.clone())
                    .unwrap_or_else(|| "Opus".to_string()),
                combined,
                format!(
                    "{}+{}/bestvideo[height<={}]+bestaudio/best[height<={}]",
                    format_id, aud_spec, height, height
                ),
                0u8,
            )
        };

        let fps_bonus = fps.unwrap_or(30) as f64 * 100.0;
        let codec_bonus = match vkind {
            "vp9" => 40.0,
            "av1" => 35.0,
            "hevc" => 30.0,
            "avc" => 25.0,
            _ => 10.0,
        };
        let split_bonus = if !is_muxed { 1000.0 } else { 0.0 };
        let hdr_bonus = if is_hdr { 15.0 } else { 0.0 };
        let score = split_bonus + fps_bonus + codec_bonus + hdr_bonus + tbr.min(9000.0) / 100.0;

        let res_name = resolution_tag(height);
        let fps_suffix = fps
            .filter(|&f| f >= 48)
            .map(|f| format!(" {}fps", f))
            .unwrap_or_default();
        let label = if bucket_tier == 1 && !container.eq_ignore_ascii_case("mkv") {
            format!("{}{} (MP4)", res_name, fps_suffix)
        } else {
            format!("{}{}", res_name, fps_suffix)
        };

        let option = OmnigetFormatOption {
            id: format!("video_{}_{}_{}", height, vkind, format_id),
            format_spec,
            label,
            resolution: res_name,
            height,
            fps,
            container,
            vcodec: vlabel,
            acodec: alabel,
            filesize_bytes: combined_size,
            audio_only: false,
            recommended: false,
            direct_url: None,
        };

        let bucket_key = (std::cmp::Reverse(height), bucket_tier);
        let replace = match best_by_bucket.get(&bucket_key) {
            Some((prev_score, _)) => score > *prev_score,
            None => true,
        };
        if replace {
            best_by_bucket.insert(bucket_key, (score, option));
        }
    }

    let mut options: Vec<OmnigetFormatOption> = Vec::new();
    let mut seen_res_containers: HashSet<(u32, String)> = HashSet::new();

    for ((_rev_h, _tier), (_score, opt)) in best_by_bucket {
        // Keep up to 2 variants per resolution (e.g., high-res WebM/MKV + compatible MP4 if distinct)
        if seen_res_containers.insert((opt.height, opt.container.clone())) {
            options.push(opt);
        }
    }

    if options.is_empty() {
        options.push(OmnigetFormatOption {
            id: "video_best_auto".to_string(),
            format_spec: "bestvideo+bestaudio/best".to_string(),
            label: "Best Available Quality".to_string(),
            resolution: "Auto".to_string(),
            height: 1080,
            fps: None,
            container: "mkv".to_string(),
            vcodec: "Auto".to_string(),
            acodec: "Auto".to_string(),
            filesize_bytes: None,
            audio_only: false,
            recommended: true,
            direct_url: None,
        });
    } else {
        options[0].recommended = true;
    }

    // 3. Append clean Audio-Only presets
    if let Some(ref aac) = best_aac {
        options.push(OmnigetFormatOption {
            id: "audio_m4a".to_string(),
            format_spec: format!("{}/bestaudio[ext=m4a]/bestaudio", aac.format_id),
            label: "Audio Only — M4A (AAC)".to_string(),
            resolution: "Audio".to_string(),
            height: 0,
            fps: None,
            container: "m4a".to_string(),
            vcodec: "Audio Only".to_string(),
            acodec: format!("AAC (~{}k)", aac.abr.round() as u32),
            filesize_bytes: aac.size_bytes,
            audio_only: true,
            recommended: false,
            direct_url: None,
        });
    } else {
        options.push(OmnigetFormatOption {
            id: "audio_m4a".to_string(),
            format_spec: "bestaudio[ext=m4a]/bestaudio".to_string(),
            label: "Audio Only — M4A (AAC)".to_string(),
            resolution: "Audio".to_string(),
            height: 0,
            fps: None,
            container: "m4a".to_string(),
            vcodec: "Audio Only".to_string(),
            acodec: "AAC".to_string(),
            filesize_bytes: None,
            audio_only: true,
            recommended: false,
            direct_url: None,
        });
    }

    options.push(OmnigetFormatOption {
        id: "audio_opus".to_string(),
        format_spec: best_opus
            .as_ref()
            .map(|a| format!("{}/bestaudio[acodec*=opus]/bestaudio", a.format_id))
            .unwrap_or_else(|| "bestaudio[acodec*=opus]/bestaudio".to_string()),
        label: "Audio Only — Opus".to_string(),
        resolution: "Audio".to_string(),
        height: 0,
        fps: None,
        container: "opus".to_string(),
        vcodec: "Audio Only".to_string(),
        acodec: best_opus
            .as_ref()
            .map(|a| format!("Opus (~{}k)", a.abr.round() as u32))
            .unwrap_or_else(|| "Opus".to_string()),
        filesize_bytes: best_opus.as_ref().and_then(|a| a.size_bytes),
        audio_only: true,
        recommended: false,
        direct_url: None,
    });

    options.push(OmnigetFormatOption {
        id: "audio_mp3".to_string(),
        format_spec: "bestaudio/best".to_string(),
        label: "Audio Only — MP3".to_string(),
        resolution: "Audio".to_string(),
        height: 0,
        fps: None,
        container: "mp3".to_string(),
        vcodec: "Audio Only".to_string(),
        acodec: "MP3 (320k)".to_string(),
        filesize_bytes: best_any_audio.as_ref().and_then(|a| a.size_bytes),
        audio_only: true,
        recommended: false,
        direct_url: None,
    });

    Ok(OmnigetMediaInfo {
        url: canonical_url,
        title,
        uploader,
        thumbnail,
        duration_seconds,
        duration_formatted,
        extractor,
        is_direct_file: false,
        formats: options,
    })
}

async fn try_direct_http_probe(
    req: &OmnigetFetchRequest,
    page_url: &str,
) -> Option<OmnigetMediaInfo> {
    let mut candidates = Vec::new();
    if let Some(ref c) = req.candidate_src {
        if let Ok(valid) = validate_http_url(c) {
            candidates.push(valid);
        }
    }
    candidates.push(page_url.to_string());

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .user_agent(
            req.user_agent
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(DEFAULT_BROWSER_USER_AGENT),
        )
        .build()
        .ok()?;

    for candidate in candidates {
        let mut rb = client.head(&candidate);
        if let Some(ref r) = req.referrer {
            if !r.trim().is_empty() {
                rb = rb.header("Referer", r);
            }
        }
        if let Some(ref ck) = req.cookies {
            if !ck.trim().is_empty() {
                rb = rb.header("Cookie", ck);
            }
        }

        let resp = match rb.send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        let looks_like_media = ctype.starts_with("video/")
            || ctype.starts_with("audio/")
            || candidate.to_ascii_lowercase().ends_with(".mp4")
            || candidate.to_ascii_lowercase().ends_with(".mkv")
            || candidate.to_ascii_lowercase().ends_with(".webm")
            || candidate.to_ascii_lowercase().ends_with(".mov");

        if !looks_like_media {
            continue;
        }

        let clen = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&n| n > 0);

        let is_audio = ctype.starts_with("audio/");
        let ext = if ctype.contains("webm") || candidate.to_ascii_lowercase().ends_with(".webm") {
            "webm"
        } else if ctype.contains("matroska") || candidate.to_ascii_lowercase().ends_with(".mkv") {
            "mkv"
        } else if is_audio {
            "m4a"
        } else {
            "mp4"
        };

        let parsed = Url::parse(&candidate).ok();
        let file_stem = parsed
            .as_ref()
            .and_then(|u| u.path_segments())
            .and_then(|mut s| s.next_back())
            .filter(|s| !s.is_empty())
            .unwrap_or("Direct Video")
            .split('.')
            .next()
            .unwrap_or("Direct Video")
            .to_string();

        return Some(OmnigetMediaInfo {
            url: candidate.clone(),
            title: file_stem,
            uploader: parsed.and_then(|u| u.host_str().map(str::to_string)),
            thumbnail: None,
            duration_seconds: None,
            duration_formatted: None,
            extractor: Some("Direct HTTP".to_string()),
            is_direct_file: true,
            formats: vec![OmnigetFormatOption {
                id: "direct_original".to_string(),
                format_spec: "best".to_string(),
                label: "Direct Stream (Original)".to_string(),
                resolution: if is_audio {
                    "Audio".to_string()
                } else {
                    "Original".to_string()
                },
                height: if is_audio { 0 } else { 1080 },
                fps: None,
                container: ext.to_string(),
                vcodec: if is_audio {
                    "Audio Only".to_string()
                } else {
                    ctype.clone()
                },
                acodec: "Included".to_string(),
                filesize_bytes: clen,
                audio_only: is_audio,
                recommended: true,
                direct_url: Some(candidate),
            }],
        });
    }

    None
}

#[allow(dead_code)]
pub fn summarize_cookie_count(entries: Option<&[OmnigetCookieEntry]>, header: Option<&str>) -> usize {
    if let Some(e) = entries {
        if !e.is_empty() {
            return e.len();
        }
    }
    header
        .unwrap_or("")
        .split(';')
        .filter(|s| s.contains('='))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_url_blocks_blob() {
        assert!(validate_http_url("blob:https://www.youtube.com/abcd").is_err());
        assert!(validate_http_url("https://www.youtube.com/watch?v=abc").is_ok());
    }

    #[test]
    fn test_parse_youtube_4k_formats_and_smart_containers() {
        let sample = json!({
            "id": "demo4k",
            "title": "Sample 4K HDR Nature",
            "uploader": "Earth Channel",
            "duration": 600.0,
            "extractor_key": "Youtube",
            "webpage_url": "https://www.youtube.com/watch?v=demo4k",
            "formats": [
                { "format_id": "sb0", "vcodec": "none", "acodec": "none", "protocol": "mhtml" },
                { "format_id": "18", "ext": "mp4", "height": 360, "width": 640, "vcodec": "avc1.42001E", "acodec": "mp4a.40.2", "filesize": 12000000 },
                { "format_id": "137", "ext": "mp4", "height": 1080, "width": 1920, "fps": 30, "vcodec": "avc1.640028", "acodec": "none", "filesize": 120000000 },
                { "format_id": "248", "ext": "webm", "height": 1080, "width": 1920, "fps": 60, "vcodec": "vp09.00.41.08", "acodec": "none", "filesize": 135000000 },
                { "format_id": "271", "ext": "webm", "height": 1440, "width": 2560, "fps": 60, "vcodec": "vp09.00.50.08", "acodec": "none", "filesize": 280000000 },
                { "format_id": "315", "ext": "webm", "height": 2160, "width": 3840, "fps": 60, "vcodec": "vp09.00.51.08", "acodec": "none", "filesize": 640000000 },
                { "format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "abr": 129.0, "filesize": 9700000 },
                { "format_id": "251", "ext": "webm", "vcodec": "none", "acodec": "opus", "abr": 160.0, "filesize": 11500000 }
            ]
        });

        let info = parse_ytdlp_json_to_media_info(&sample, "https://www.youtube.com/watch?v=demo4k")
            .expect("Failed parsing synthetic yt-dlp JSON");

        assert_eq!(info.title, "Sample 4K HDR Nature");
        assert_eq!(info.duration_formatted.as_deref(), Some("10:00"));

        // First option must be 2160p 4K 60fps VP9 + Opus in WebM (NOT forced MP4!)
        let top = &info.formats[0];
        assert_eq!(top.height, 2160);
        assert_eq!(top.container, "webm");
        assert_eq!(top.vcodec, "VP9");
        assert_eq!(top.acodec, "Opus");
        assert!(top.recommended);
        assert_eq!(top.filesize_bytes, Some(640_000_000 + 11_500_000));
        assert!(top.format_spec.starts_with("315+251/"));

        // Should offer both 1080p VP9 (.webm) and 1080p H.264 (.mp4)
        assert!(info.formats.iter().any(|f| f.height == 1080 && f.container == "mp4" && f.vcodec == "H.264 / AVC"));
        assert!(info.formats.iter().any(|f| f.height == 1080 && f.container == "webm" && f.vcodec == "VP9"));

        // Must include M4A, Opus, MP3 audio presets
        assert!(info.formats.iter().any(|f| f.audio_only && f.container == "m4a"));
        assert!(info.formats.iter().any(|f| f.audio_only && f.container == "opus"));
        assert!(info.formats.iter().any(|f| f.audio_only && f.container == "mp3"));
    }
}

