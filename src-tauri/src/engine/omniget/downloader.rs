use crate::engine::chunk_worker::DEFAULT_BROWSER_USER_AGENT;
use crate::engine::download_engine::DownloadEngine;
use crate::engine::omniget::cookies::build_temp_cookie_file;
use crate::engine::omniget::extractor::validate_http_url;
use crate::engine::omniget::process::{build_tokio_command, kill_process_tree, ResolvedTools};
use crate::engine::omniget::{OmnigetCookieEntry, OmnigetDownloadRequest};
use crate::engine::types::{
    DownloadCategory, DownloadProgressPayload, DownloadStatus, DownloadTask, MediaStreamInfo,
    SegmentProgress, SegmentStatus,
};
use chrono::Utc;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tracing::{error, info, warn};

const OUTPUT_FILEPATH_MARKER: &str = "OCTO_FILEPATH:";
const PROGRESS_MARKER: &str = "OCTO_PROG|";
const POSTPROCESS_MARKER: &str = "OCTO_POST|";

#[derive(Debug, Clone)]
pub struct VerifiedMediaProbe {
    pub file_size: u64,
    pub duration_secs: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedProgressLine {
    pub status: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: Option<u64>,
    pub ext: Option<String>,
}

pub fn parse_octo_prog_line(line: &str) -> Option<ParsedProgressLine> {
    let payload = line.trim().strip_prefix(PROGRESS_MARKER)?;
    let parts: Vec<&str> = payload.split('|').collect();
    if parts.len() < 7 {
        return None;
    }

    let parse_u64 = |s: &str| -> u64 {
        let t = s.trim();
        if t.is_empty() || t == "NA" || t == "None" {
            return 0;
        }
        t.parse::<f64>().map(|v| v.max(0.0).round() as u64).unwrap_or(0)
    };

    let status = parts[0].trim().to_ascii_lowercase();
    let downloaded_bytes = parse_u64(parts[1]);
    let total_exact = parse_u64(parts[2]);
    let total_est = parse_u64(parts[3]);
    let total_bytes = if total_exact > 0 {
        total_exact
    } else {
        total_est
    };
    let speed_bytes_per_sec = parse_u64(parts[4]);
    let eta_raw = parse_u64(parts[5]);
    let eta_seconds = if eta_raw > 0 || parts[5].trim() == "0" {
        Some(eta_raw)
    } else {
        None
    };
    let ext = parts
        .get(6)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "NA" && *s != "None")
        .map(str::to_string);

    Some(ParsedProgressLine {
        status,
        downloaded_bytes,
        total_bytes,
        speed_bytes_per_sec,
        eta_seconds,
        ext,
    })
}

pub fn parse_octo_post_line(line: &str) -> Option<(String, String)> {
    let payload = line.trim().strip_prefix(POSTPROCESS_MARKER)?;
    let (status, postprocessor) = payload.split_once('|')?;
    Some((
        status.trim().to_ascii_lowercase(),
        postprocessor.trim().to_string(),
    ))
}

pub fn sanitize_file_stem(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => ' ',
            _ => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().trim_matches('.');
    let truncated: String = trimmed.chars().take(120).collect();
    let final_stem = truncated.trim().trim_matches('.');
    if final_stem.is_empty() {
        fallback.to_string()
    } else {
        final_stem.to_string()
    }
}

fn strip_known_media_ext(name: &str) -> &str {
    let trimmed = name.trim();
    if let Some((stem, ext)) = trimmed.rsplit_once('.') {
        if matches!(
            ext.to_ascii_lowercase().as_str(),
            "mp4" | "mkv" | "webm" | "mov" | "m4v" | "m4a" | "opus" | "mp3" | "ogg" | "flac"
        ) {
            return stem;
        }
    }
    trimmed
}

fn resolve_non_colliding_stem(dest_dir: &Path, base_stem: &str, container: &str) -> String {
    let primary = dest_dir.join(format!("{}.{}", base_stem, container));
    if !primary.exists() {
        return base_stem.to_string();
    }
    for idx in 2..=99 {
        let candidate_stem = format!("{} ({})", base_stem, idx);
        let candidate_path = dest_dir.join(format!("{}.{}", candidate_stem, container));
        if !candidate_path.exists() {
            return candidate_stem;
        }
    }
    format!(
        "{}_{}",
        base_stem,
        Utc::now().format("%Y%m%d_%H%M%S")
    )
}

fn normalize_container(container: &str, audio_only: bool) -> String {
    let c = container.trim().trim_start_matches('.').to_ascii_lowercase();
    if audio_only {
        match c.as_str() {
            "m4a" | "opus" | "mp3" | "ogg" | "flac" | "wav" => c,
            _ => "m4a".to_string(),
        }
    } else {
        match c.as_str() {
            "mp4" | "webm" | "mkv" => c,
            _ => "mkv".to_string(),
        }
    }
}

/// Starts a managed OctoMedia (`yt-dlp` + `ffmpeg`) download as a first-class `DownloadTask`.
pub fn start_omniget_download(
    req: OmnigetDownloadRequest,
    engine: Arc<DownloadEngine>,
    app: Option<AppHandle>,
) -> Result<DownloadTask, String> {
    let clean_url = validate_http_url(&req.url)?;
    let tools = ResolvedTools::discover();
    if tools.ytdlp_path.is_none() {
        return Err(
            "yt-dlp is not installed yet. Click 'Auto-Setup Tools' in OctoMedia first."
                .to_string(),
        );
    }

    let category = if req.audio_only {
        DownloadCategory::Audio
    } else {
        DownloadCategory::Video
    };

    let target_dir = req
        .save_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| engine.category_dispatcher.resolve_destination_dir(&category));
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed creating destination folder: {}", e))?;

    let container = normalize_container(&req.container, req.audio_only);
    let raw_name = req
        .file_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| req.title.as_deref().filter(|s| !s.trim().is_empty()))
        .unwrap_or("video");
    let mut base_stem = sanitize_file_stem(strip_known_media_ext(raw_name), "video");
    if !req.audio_only {
        if let Some(ref res_label) = req.resolution {
            if let Some(short_p) = res_label.split_whitespace().next() {
                if short_p.ends_with('p')
                    && !base_stem
                        .to_ascii_lowercase()
                        .contains(&short_p.to_ascii_lowercase())
                {
                    base_stem = format!("{} - {}", base_stem, short_p);
                }
            }
        }
    }

    let final_stem = resolve_non_colliding_stem(&target_dir, &base_stem, &container);
    let file_name = format!("{}.{}", final_stem, container);
    let save_path = target_dir.join(&file_name);

    let est_size = req.estimated_bytes.unwrap_or(0);
    let task_id = uuid::Uuid::new_v4().to_string();

    let media_info = MediaStreamInfo {
        protocol: "YT-DLP".to_string(),
        video_resolution: req.resolution.clone().or_else(|| {
            if req.audio_only {
                Some("Audio".to_string())
            } else {
                None
            }
        }),
        video_codec: req.vcodec.clone(),
        audio_codec: req.acodec.clone(),
        audio_bitrate: None,
        total_segments: if req.audio_only { 1 } else { 2 },
        completed_segments: 0,
        mux_phase: "downloading".to_string(),
        mux_percent: 0.0,
        format_spec: Some(req.format_spec.clone()),
        source_url: Some(clean_url.clone()),
        target_container: Some(container.clone()),
        audio_only: req.audio_only,
    };

    let task = DownloadTask {
        id: task_id.clone(),
        url: clean_url.clone(),
        file_name: file_name.clone(),
        save_path: save_path.to_string_lossy().to_string(),
        total_size: est_size,
        downloaded_bytes: 0,
        speed: 0,
        eta_seconds: None,
        status: DownloadStatus::Connecting,
        category,
        supports_range: true,
        connections: 8,
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
        headers: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    engine.state_store.add_task(task.clone());
    if let Some(ref a) = app {
        let _ = a.emit("tasks-updated", &task);
    }

    let cancel_rx = engine.register_external_cancel(&task_id);
    let output_template = target_dir.join(format!("{}.%(ext)s", final_stem));

    let job = OmnigetJobParams {
        task_id,
        source_url: clean_url,
        direct_url: req.direct_url,
        format_spec: req.format_spec,
        container,
        audio_only: req.audio_only,
        estimated_bytes: est_size,
        output_template,
        expected_output_path: save_path,
        media_info,
        cookies: req.cookies,
        cookie_entries: req.cookie_entries,
        user_agent: req.user_agent,
        referrer: req.referrer,
    };

    spawn_bg(async move {
        run_omniget_job(job, engine, app, cancel_rx).await;
    });

    Ok(task)
}

/// Resumes a paused or interrupted OctoMedia (`YT-DLP`) task cleanly using `--continue`.
pub fn resume_omniget_task(
    task_id: &str,
    engine: Arc<DownloadEngine>,
    app: Option<AppHandle>,
) -> Result<(), String> {
    let task = engine
        .state_store
        .get_task(task_id)
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    if task.status == DownloadStatus::Completed {
        return Ok(());
    }

    let mi = task
        .media_info
        .clone()
        .ok_or_else(|| "Task missing OctoMedia stream metadata".to_string())?;
    let expected_path = PathBuf::from(&task.save_path);
    let parent_dir = expected_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let stem = expected_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video")
        .to_string();
    let output_template = parent_dir.join(format!("{}.%(ext)s", stem));
    let container = mi
        .target_container
        .clone()
        .unwrap_or_else(|| if mi.audio_only { "m4a" } else { "mkv" }.to_string());
    let source_url = mi.source_url.clone().unwrap_or_else(|| task.url.clone());
    let format_spec = mi
        .format_spec
        .clone()
        .unwrap_or_else(|| "bestvideo+bestaudio/best".to_string());

    engine
        .state_store
        .update_task_status(task_id, DownloadStatus::Connecting, None);
    let cancel_rx = engine.register_external_cancel(task_id);

    let job = OmnigetJobParams {
        task_id: task.id,
        source_url,
        direct_url: None,
        format_spec,
        container,
        audio_only: mi.audio_only,
        estimated_bytes: task.total_size,
        output_template,
        expected_output_path: expected_path,
        media_info: mi,
        cookies: None,
        cookie_entries: None,
        user_agent: None,
        referrer: None,
    };

    spawn_bg(async move {
        run_omniget_job(job, engine, app, cancel_rx).await;
    });

    Ok(())
}

/// Standalone awaitable execution helper used for headless integration verification & live 4K tests.
pub async fn download_media_direct_to_path(
    req: &OmnigetDownloadRequest,
    dest_dir: &Path,
) -> Result<(PathBuf, VerifiedMediaProbe), String> {
    let clean_url = validate_http_url(&req.url)?;
    let tools = ResolvedTools::discover();
    if tools.ytdlp_path.is_none() {
        return Err("yt-dlp executable was not found".to_string());
    }
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;

    let container = normalize_container(&req.container, req.audio_only);
    let raw_name = req
        .file_name
        .as_deref()
        .or(req.title.as_deref())
        .unwrap_or("test_video");
    let base_stem = sanitize_file_stem(strip_known_media_ext(raw_name), "test_video");
    let stem = resolve_non_colliding_stem(dest_dir, &base_stem, &container);
    let output_template = dest_dir.join(format!("{}.%(ext)s", stem));
    let expected_path = dest_dir.join(format!("{}.{}", stem, container));

    let temp_cookies = build_temp_cookie_file(
        &clean_url,
        req.cookie_entries.as_deref(),
        req.cookies.as_deref(),
    )?;

    let (_dummy_tx, mut cancel_rx) = watch::channel(false);
    let outcome = execute_ytdlp_attempt(
        &tools,
        req.direct_url.as_deref().unwrap_or(&clean_url),
        &req.format_spec,
        &container,
        req.audio_only,
        req.direct_url.is_some(),
        &output_template,
        temp_cookies.as_ref().map(|c| c.path()),
        req.user_agent.as_deref(),
        req.referrer.as_deref(),
        None,
        &mut cancel_rx,
        |_, _, _, _, _| {},
    )
    .await?;

    let final_path = outcome.reported_filepath.unwrap_or(expected_path);
    let probe = verify_media_container(&tools, &final_path, req.audio_only).await?;
    Ok((final_path, probe))
}

struct OmnigetJobParams {
    task_id: String,
    source_url: String,
    direct_url: Option<String>,
    format_spec: String,
    container: String,
    audio_only: bool,
    estimated_bytes: u64,
    output_template: PathBuf,
    expected_output_path: PathBuf,
    media_info: MediaStreamInfo,
    cookies: Option<String>,
    cookie_entries: Option<Vec<OmnigetCookieEntry>>,
    user_agent: Option<String>,
    referrer: Option<String>,
}

struct AttemptOutput {
    reported_filepath: Option<PathBuf>,
    cancelled: bool,
}

async fn run_omniget_job(
    job: OmnigetJobParams,
    engine: Arc<DownloadEngine>,
    app: Option<AppHandle>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let tools = ResolvedTools::discover();
    let Some(_) = tools.ytdlp_path else {
        fail_task(
            &job.task_id,
            "yt-dlp executable missing; run OctoMedia auto-setup.",
            &engine,
            app.as_ref(),
        );
        engine.unregister_external_control(&job.task_id);
        return;
    };

    let temp_cookies = match build_temp_cookie_file(
        &job.source_url,
        job.cookie_entries.as_deref(),
        job.cookies.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            fail_task(&job.task_id, &e, &engine, app.as_ref());
            engine.unregister_external_control(&job.task_id);
            return;
        }
    };

    engine
        .state_store
        .update_task_status(&job.task_id, DownloadStatus::Downloading, None);

    let last_emit = Arc::new(parking_lot::Mutex::new(
        Instant::now() - Duration::from_secs(1),
    ));
    let mut current_media = job.media_info.clone();
    let target_url = job.direct_url.as_deref().unwrap_or(&job.source_url);
    let is_direct = job.direct_url.is_some();

    let strategies: Vec<(Option<&str>, bool)> = vec![
        (None, true),
        (
            Some("youtube:player_client=default,web_safari,mweb;formats=dashy"),
            true,
        ),
        (
            Some("youtube:player_client=default,web_safari,mweb;formats=dashy"),
            false,
        ),
    ];

    let mut final_attempt: Result<AttemptOutput, String> = Err("Download not started".to_string());

    for (idx, (extractor_args, use_cookies)) in strategies.into_iter().enumerate() {
        if *cancel_rx.borrow() {
            final_attempt = Ok(AttemptOutput {
                reported_filepath: None,
                cancelled: true,
            });
            break;
        }
        if idx == 2 && temp_cookies.is_none() {
            continue;
        }
        if idx > 0 {
            info!(
                task_id = %job.task_id,
                strategy_idx = idx,
                "Retrying OctoMedia download with fallback client strategy"
            );
        }

        let cookie_path = if use_cookies {
            temp_cookies.as_ref().map(|c| c.path())
        } else {
            None
        };

        let store = Arc::clone(&engine.state_store);
        let tid = job.task_id.clone();
        let app_ref = app.clone();
        let last_emit_c = Arc::clone(&last_emit);
        let est_total = job.estimated_bytes;
        let mut media_snapshot = current_media.clone();

        let res = execute_ytdlp_attempt(
            &tools,
            target_url,
            &job.format_spec,
            &job.container,
            job.audio_only,
            is_direct,
            &job.output_template,
            cookie_path,
            job.user_agent.as_deref(),
            job.referrer.as_deref(),
            extractor_args,
            &mut cancel_rx,
            |phase, dl, tot, spd, eta| {
                media_snapshot.mux_phase = phase.to_string();
                let effective_total = tot.max(est_total).max(dl);
                let pct = if phase == "muxing" {
                    97.0
                } else if effective_total > 0 {
                    ((dl as f64 / effective_total as f64) * 95.0).clamp(0.0, 95.0)
                } else {
                    0.0
                };
                media_snapshot.mux_percent = pct;
                if phase == "muxing" {
                    media_snapshot.completed_segments = media_snapshot.total_segments;
                }

                let force = phase == "muxing";
                let should_emit = force || {
                    let mut g = last_emit_c.lock();
                    if g.elapsed() >= Duration::from_millis(160) {
                        *g = Instant::now();
                        true
                    } else {
                        false
                    }
                };

                if should_emit {
                    let segs = vec![SegmentProgress {
                        id: 0,
                        start_byte: 0,
                        end_byte: effective_total.saturating_sub(1),
                        current_byte: dl,
                        downloaded: dl,
                        speed: spd,
                        status: SegmentStatus::Downloading,
                    }];
                    if effective_total > 0 {
                        store.update_total_and_connections(&tid, effective_total, 8);
                    }
                    store.update_progress(&tid, dl, spd, eta, segs.clone());
                    store.update_task_media_info(&tid, media_snapshot.clone());

                    if let Some(ref a) = app_ref {
                        let _ = a.emit(
                            "download-progress",
                            &DownloadProgressPayload {
                                id: tid.clone(),
                                status: DownloadStatus::Downloading,
                                downloaded_bytes: dl,
                                total_size: effective_total,
                                speed: spd,
                                eta_seconds: eta,
                                segments: segs,
                                error_message: None,
                                media_info: Some(media_snapshot.clone()),
                            },
                        );
                    }
                }
            },
        )
        .await;

        current_media = media_snapshot;

        match res {
            Ok(out) => {
                final_attempt = Ok(out);
                break;
            }
            Err(err) => {
                warn!(task_id = %job.task_id, error = %err, "yt-dlp attempt failed");
                final_attempt = Err(err);
                if is_direct {
                    break;
                }
            }
        }
    }

    if !matches!(final_attempt, Ok(AttemptOutput { cancelled: true, .. })) {
        engine.unregister_external_control(&job.task_id);
    }

    match final_attempt {
        Ok(out) if out.cancelled => {
            if let Some(existing) = engine.state_store.get_task(&job.task_id) {
                if existing.status != DownloadStatus::Connecting
                    && existing.status != DownloadStatus::Downloading
                {
                    engine
                        .state_store
                        .update_task_status(&job.task_id, DownloadStatus::Paused, None);
                    if let Some(ref a) = app {
                        let _ = a.emit(
                            "download-progress",
                            &DownloadProgressPayload {
                                id: job.task_id.clone(),
                                status: DownloadStatus::Paused,
                                downloaded_bytes: existing.downloaded_bytes,
                                total_size: existing.total_size,
                                speed: 0,
                                eta_seconds: None,
                                segments: existing.segments,
                                error_message: None,
                                media_info: existing.media_info,
                            },
                        );
                    }
                }
            }
        }
        Ok(out) => {
            let final_path = resolve_final_output_path(
                out.reported_filepath.as_deref(),
                &job.expected_output_path,
            );
            match verify_media_container(&tools, &final_path, job.audio_only).await {
                Ok(probe) => {
                    let final_len = probe.file_size;
                    if let Some(mut updated) = engine.state_store.get_task(&job.task_id) {
                        if let Some(fname) = final_path.file_name().and_then(|n| n.to_str()) {
                            updated.file_name = fname.to_string();
                        }
                        updated.save_path = final_path.to_string_lossy().to_string();
                        updated.total_size = final_len;
                        updated.downloaded_bytes = final_len;
                        updated.speed = 0;
                        updated.eta_seconds = Some(0);
                        updated.status = DownloadStatus::Completed;
                        updated.error_message = None;
                        updated.segments = vec![SegmentProgress {
                            id: 0,
                            start_byte: 0,
                            end_byte: final_len.saturating_sub(1),
                            current_byte: final_len,
                            downloaded: final_len,
                            speed: 0,
                            status: SegmentStatus::Completed,
                        }];
                        if let Some(ref mut mi) = updated.media_info {
                            mi.mux_phase = "completed".to_string();
                            mi.mux_percent = 100.0;
                            mi.completed_segments = mi.total_segments;
                            if let (Some(w), Some(h)) = (probe.video_width, probe.video_height) {
                                mi.video_resolution = Some(format!("{}x{}", w, h));
                            }
                            if let Some(ref vc) = probe.video_codec {
                                mi.video_codec = Some(vc.to_ascii_uppercase());
                            }
                            if let Some(ref ac) = probe.audio_codec {
                                mi.audio_codec = Some(ac.to_ascii_uppercase());
                            }
                        }
                        updated.updated_at = Utc::now();
                        engine.state_store.add_task(updated.clone());

                        if let Some(ref a) = app {
                            let _ = a.emit("tasks-updated", &updated);
                            let _ = a.emit(
                                "download-progress",
                                &DownloadProgressPayload {
                                    id: job.task_id.clone(),
                                    status: DownloadStatus::Completed,
                                    downloaded_bytes: final_len,
                                    total_size: final_len,
                                    speed: 0,
                                    eta_seconds: Some(0),
                                    segments: updated.segments.clone(),
                                    error_message: None,
                                    media_info: updated.media_info.clone(),
                                },
                            );
                        }
                    }
                    info!(
                        task_id = %job.task_id,
                        path = %final_path.display(),
                        bytes = final_len,
                        "OctoMedia download & mux verified cleanly"
                    );
                }
                Err(verify_err) => {
                    fail_task(&job.task_id, &verify_err, &engine, app.as_ref());
                }
            }
        }
        Err(err) => {
            fail_task(&job.task_id, &err, &engine, app.as_ref());
        }
    }
}

fn fail_task(
    task_id: &str,
    error_msg: &str,
    engine: &DownloadEngine,
    app: Option<&AppHandle>,
) {
    error!(task_id = %task_id, error = %error_msg, "OctoMedia task failed");
    engine
        .state_store
        .update_task_status(task_id, DownloadStatus::Error, Some(error_msg.to_string()));
    if let Some(existing) = engine.state_store.get_task(task_id) {
        if let Some(a) = app {
            let _ = a.emit("tasks-updated", &existing);
            let _ = a.emit(
                "download-progress",
                &DownloadProgressPayload {
                    id: task_id.to_string(),
                    status: DownloadStatus::Error,
                    downloaded_bytes: existing.downloaded_bytes,
                    total_size: existing.total_size,
                    speed: 0,
                    eta_seconds: None,
                    segments: existing.segments,
                    error_message: Some(error_msg.to_string()),
                    media_info: existing.media_info,
                },
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_ytdlp_attempt<F>(
    tools: &ResolvedTools,
    url: &str,
    format_spec: &str,
    container: &str,
    audio_only: bool,
    is_direct: bool,
    output_template: &Path,
    cookie_path: Option<&Path>,
    user_agent: Option<&str>,
    referrer: Option<&str>,
    extractor_args: Option<&str>,
    cancel_rx: &mut watch::Receiver<bool>,
    mut on_progress: F,
) -> Result<AttemptOutput, String>
where
    F: FnMut(&str, u64, u64, u64, Option<u64>),
{
    let ytdlp_path = tools
        .ytdlp_path
        .as_ref()
        .ok_or_else(|| "yt-dlp missing".to_string())?;

    let mut cmd = build_tokio_command(ytdlp_path, tools);
    cmd.args([
        "--ignore-config",
        "--no-embed-info-json",
        "--continue",
        "--no-playlist",
        "--encoding",
        "utf-8",
        "--newline",
        "--progress",
        "--progress-delta",
        "0.25",
        "--progress-template",
        "download:OCTO_PROG|%(progress.status)s|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(info.ext)s",
        "--progress-template",
        "postprocess:OCTO_POST|%(progress.status)s|%(progress.postprocessor)s",
        "--print",
        "after_move:OCTO_FILEPATH:%(filepath)s",
        "-N",
        "8",
    ]);

    if let Some(ref ffmpeg) = tools.ffmpeg_path {
        if let Some(parent) = ffmpeg.parent() {
            cmd.args(["--ffmpeg-location", &parent.to_string_lossy()]);
        }
    }
    if let Some((kind, js_path)) = &tools.js_runtime {
        cmd.args(["--js-runtimes", &format!("{}:{}", kind, js_path.display())]);
    }

    let ua = user_agent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BROWSER_USER_AGENT);
    cmd.args(["--user-agent", ua]);

    if let Some(r) = referrer.map(str::trim).filter(|s| !s.is_empty()) {
        cmd.args(["--referer", r]);
    }
    if let Some(cp) = cookie_path {
        cmd.args(["--cookies", &cp.to_string_lossy()]);
    }
    if let Some(ea) = extractor_args {
        cmd.args(["--extractor-args", ea]);
    }

    if !is_direct {
        if audio_only {
            cmd.args(["-f", format_spec, "-x", "--audio-format", container, "--audio-quality", "0"]);
        } else {
            let merge_fmt = match container {
                "mp4" => "mp4/mkv",
                "webm" => "webm/mkv",
                _ => "mkv",
            };
            cmd.args(["-f", format_spec, "--merge-output-format", merge_fmt]);
        }
    }

    cmd.args(["-o", &output_template.to_string_lossy(), url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed starting yt-dlp process: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed opening yt-dlp stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed opening yt-dlp stderr".to_string())?;

    let stderr_lines = Arc::new(parking_lot::Mutex::new(VecDeque::<String>::with_capacity(64)));
    let stderr_ring = Arc::clone(&stderr_lines);
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                let mut g = stderr_ring.lock();
                if g.len() >= 64 {
                    g.pop_front();
                }
                g.push_back(trimmed);
            }
        }
    });

    let mut out_reader = BufReader::new(stdout).lines();
    let mut reported_filepath: Option<PathBuf> = None;
    let mut prior_completed_bytes: u64 = 0;
    let mut current_stream_dl: u64 = 0;
    let mut current_stream_total: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    kill_process_tree(&mut child).await;
                    let _ = stderr_task.await;
                    return Ok(AttemptOutput {
                        reported_filepath: None,
                        cancelled: true,
                    });
                }
            }
            line_res = out_reader.next_line() => {
                match line_res {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if let Some(fp) = trimmed.strip_prefix(OUTPUT_FILEPATH_MARKER) {
                            let clean_fp = fp.trim();
                            if !clean_fp.is_empty() && clean_fp != "NA" {
                                reported_filepath = Some(PathBuf::from(clean_fp));
                            }
                            continue;
                        }
                        if let Some(prog) = parse_octo_prog_line(trimmed) {
                            if prog.status == "finished" {
                                prior_completed_bytes += current_stream_dl.max(prog.downloaded_bytes).max(prog.total_bytes);
                                current_stream_dl = 0;
                                current_stream_total = 0;
                                on_progress("downloading", prior_completed_bytes, prior_completed_bytes, 0, None);
                            } else {
                                current_stream_dl = prog.downloaded_bytes;
                                if prog.total_bytes > 0 {
                                    current_stream_total = prog.total_bytes;
                                }
                                let combined_dl = prior_completed_bytes + current_stream_dl;
                                let combined_tot = prior_completed_bytes + current_stream_total;
                                on_progress(
                                    "downloading",
                                    combined_dl,
                                    combined_tot,
                                    prog.speed_bytes_per_sec,
                                    prog.eta_seconds,
                                );
                            }
                            continue;
                        }
                        if let Some((_st, _pp)) = parse_octo_post_line(trimmed) {
                            let combined = prior_completed_bytes.max(1);
                            on_progress("muxing", combined, combined, 0, Some(0));
                            continue;
                        }
                        if trimmed.starts_with("[Merger]") || trimmed.starts_with("[ExtractAudio]") {
                            let combined = prior_completed_bytes.max(1);
                            on_progress("muxing", combined, combined, 0, Some(0));
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("yt-dlp stdout read error: {}", e);
                        break;
                    }
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Failed awaiting yt-dlp process: {}", e))?;
    let _ = stderr_task.await;

    if *cancel_rx.borrow() {
        return Ok(AttemptOutput {
            reported_filepath: None,
            cancelled: true,
        });
    }

    if !status.success() {
        let errs = stderr_lines.lock();
        let detail = errs
            .iter()
            .rfind(|l| l.contains("ERROR:"))
            .or_else(|| errs.back())
            .cloned()
            .unwrap_or_else(|| format!("yt-dlp exited with status {}", status));
        return Err(detail);
    }

    Ok(AttemptOutput {
        reported_filepath,
        cancelled: false,
    })
}

fn resolve_final_output_path(reported: Option<&Path>, expected: &Path) -> PathBuf {
    if let Some(p) = reported {
        if p.is_file() {
            return p.to_path_buf();
        }
    }
    if expected.is_file() {
        return expected.to_path_buf();
    }
    if let (Some(parent), Some(stem)) = (expected.parent(), expected.file_stem()) {
        let stem_str = stem.to_string_lossy();
        for ext in ["mkv", "webm", "mp4", "m4a", "opus", "mp3"] {
            let candidate = parent.join(format!("{}.{}", stem_str, ext));
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    expected.to_path_buf()
}

pub async fn verify_media_container(
    tools: &ResolvedTools,
    file_path: &Path,
    audio_only: bool,
) -> Result<VerifiedMediaProbe, String> {
    let meta = std::fs::metadata(file_path).map_err(|e| {
        format!(
            "Downloaded media file not found at '{}': {}",
            file_path.display(),
            e
        )
    })?;
    let file_size = meta.len();
    if file_size == 0 {
        return Err("Downloaded media file is empty (0 bytes)".to_string());
    }

    let Some(ref ffprobe_path) = tools.ffprobe_path else {
        return Ok(VerifiedMediaProbe {
            file_size,
            duration_secs: None,
            has_video: !audio_only,
            has_audio: true,
            video_width: None,
            video_height: None,
            video_codec: None,
            audio_codec: None,
        });
    };

    let mut cmd = build_tokio_command(ffprobe_path, tools);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type,codec_name,width,height",
        "-show_entries",
        "format=duration,size",
        "-of",
        "json",
    ])
    .arg(file_path)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed executing ffprobe verification: {}", e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Container validation via ffprobe failed (file may be corrupt): {}",
            err.trim()
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Invalid ffprobe JSON output: {}", e))?;

    let duration_secs = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    let mut has_video = false;
    let mut has_audio = false;
    let mut video_width = None;
    let mut video_height = None;
    let mut video_codec = None;
    let mut audio_codec = None;

    for st in &streams {
        match st.get("codec_type").and_then(|v| v.as_str()) {
            Some("video") => {
                has_video = true;
                video_width = st.get("width").and_then(|v| v.as_u64()).map(|n| n as u32);
                video_height = st.get("height").and_then(|v| v.as_u64()).map(|n| n as u32);
                video_codec = st
                    .get("codec_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            Some("audio") => {
                has_audio = true;
                audio_codec = st
                    .get("codec_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            _ => {}
        }
    }

    if audio_only && !has_audio {
        return Err("Verified container contains no valid audio stream".to_string());
    }
    if !audio_only && !has_video {
        return Err("Verified video container contains no valid video stream".to_string());
    }

    Ok(VerifiedMediaProbe {
        file_size,
        duration_secs,
        has_video,
        has_audio,
        video_width,
        video_height,
        video_codec,
        audio_codec,
    })
}

fn spawn_bg<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(fut);
    } else {
        tauri::async_runtime::spawn(fut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_octo_progress_and_post_lines() {
        let p = parse_octo_prog_line(
            "OCTO_PROG|downloading|10485760|104857600|NA|5242880.5|18|webm",
        )
        .expect("should parse valid progress line");
        assert_eq!(p.status, "downloading");
        assert_eq!(p.downloaded_bytes, 10_485_760);
        assert_eq!(p.total_bytes, 104_857_600);
        assert_eq!(p.speed_bytes_per_sec, 5_242_881);
        assert_eq!(p.eta_seconds, Some(18));
        assert_eq!(p.ext.as_deref(), Some("webm"));

        let post = parse_octo_post_line("OCTO_POST|started|Merger")
            .expect("should parse postprocess line");
        assert_eq!(post.0, "started");
        assert_eq!(post.1, "Merger");
    }

    #[test]
    fn test_sanitize_file_stem() {
        assert_eq!(
            sanitize_file_stem("My 4K: Video / Test? *clip*", "fallback"),
            "My 4K_ Video _ Test_ _clip_"
        );
        assert_eq!(sanitize_file_stem("...", "fallback"), "fallback");
    }

    /// Live 4K end-to-end verification test (run explicitly with `cargo test test_live_4k_youtube_download -- --ignored --nocapture`)
    #[tokio::test]
    #[ignore]
    async fn test_live_4k_youtube_download_and_ffprobe() {
        use crate::engine::omniget::extractor::fetch_media_formats;
        use crate::engine::omniget::OmnigetFetchRequest;
        use uuid::Uuid;

        // Long (~10 minute) 4K Creative Commons YouTube reference video ("Costa Rica in 4K 60fps" ~5m13s or "Peru 8K / 4K" / "4K Nature")
        // Note: Peru 8K HDR 60fps (~5:37) or Costa Rica 4K (~5:14) or LG Jazz / Morocco 4K (~10m) — let's use classic public 4K ~10min video:
        // "Big Buck Bunny 60fps 4K" (10:34, Creative Commons Blender Foundation: https://www.youtube.com/watch?v=aqz-KE-bpKQ)
        let test_url = "https://www.youtube.com/watch?v=aqz-KE-bpKQ";
        let fetch_req = OmnigetFetchRequest {
            url: test_url.to_string(),
            candidate_src: None,
            cookies: None,
            cookie_entries: None,
            user_agent: None,
            referrer: None,
        };

        let info = fetch_media_formats(&fetch_req)
            .await
            .expect("fetch_media_formats failed");
        println!(
            "Title: {}, Duration: {:?}, Formats: {}",
            info.title,
            info.duration_formatted,
            info.formats.len()
        );
        for f in &info.formats {
            println!(
                "  - {} | {} | {}+{} | {} | est={:?}",
                f.id, f.label, f.vcodec, f.acodec, f.container, f.filesize_bytes
            );
        }

        let chosen = info
            .formats
            .iter()
            .find(|f| !f.audio_only && f.height >= 2160)
            .unwrap_or_else(|| &info.formats[0])
            .clone();
        println!(
            "Selected format: {} ({}, {}+{}, container={})",
            chosen.label, chosen.format_spec, chosen.vcodec, chosen.acodec, chosen.container
        );
        assert!(chosen.height >= 2160, "Expected >= 2160p 4K format on 4K reference video");
        assert_ne!(
            chosen.container, "mp4",
            "VP9/AV1 4K streams must not be forced into broken AVC/MP4 container"
        );

        let temp_dir = std::env::temp_dir().join(format!("octoheft_live_4k_{}", Uuid::new_v4()));
        let dl_req = OmnigetDownloadRequest {
            url: test_url.to_string(),
            title: Some(info.title.clone()),
            format_spec: chosen.format_spec.clone(),
            format_id: Some(chosen.id.clone()),
            container: chosen.container.clone(),
            resolution: Some(chosen.resolution.clone()),
            vcodec: Some(chosen.vcodec.clone()),
            acodec: Some(chosen.acodec.clone()),
            audio_only: false,
            estimated_bytes: chosen.filesize_bytes,
            save_dir: Some(temp_dir.to_string_lossy().to_string()),
            file_name: Some("OctoMedia_Live4K_Verify".to_string()),
            cookies: None,
            cookie_entries: None,
            user_agent: None,
            referrer: None,
            direct_url: chosen.direct_url.clone(),
        };

        let (final_path, probe) = download_media_direct_to_path(&dl_req, &temp_dir)
            .await
            .expect("Live 4K download failed");
        println!(
            "Verified 4K output: {} | bytes={} | res={:?}x{:?} | v={} | a={} | dur={:?}s",
            final_path.display(),
            probe.file_size,
            probe.video_width,
            probe.video_height,
            probe.video_codec.as_deref().unwrap_or("?"),
            probe.audio_codec.as_deref().unwrap_or("?"),
            probe.duration_secs
        );

        assert!(probe.has_video);
        assert!(probe.has_audio);
        assert!(probe.video_height.unwrap_or(0) >= 2160);
        assert!(
            probe.file_size > 100 * 1024 * 1024,
            "10-min 4K video must not truncate around 46 MB; got {} bytes",
            probe.file_size
        );
        assert!(probe.duration_secs.unwrap_or(0.0) > 500.0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
