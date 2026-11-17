use crate::engine::categories::CategoryDispatcher;
use crate::engine::chunk_worker::{
    send_request_with_headers, ChunkWorker, SpeedLimiter, WorkerProgressEvent,
    DEFAULT_BROWSER_USER_AGENT, SEG_STATE_COMPLETED, SEG_STATE_DOWNLOADING, SEG_STATE_FAILED,
    SEG_STATE_PENDING,
};
use crate::engine::sparse_file::SparseFile;
use crate::engine::state_store::StateStore;
use crate::engine::telemetry::{EwmaSpeedTracker, TELEMETRY_TICK_INTERVAL};
use crate::engine::types::{
    AddDownloadRequest, DownloadProgressPayload, DownloadStatus, DownloadTask,
    SegmentProgress, SegmentStatus,
};
use chrono::Utc;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, watch};
use tokio::time::MissedTickBehavior;
use tracing::{error, info, warn};
use url::Url;

struct ActiveTaskControl {
    cancel_tx: watch::Sender<bool>,
    speed_limiter: SpeedLimiter,
}

pub struct DownloadEngine {
    client: reqwest::Client,
    pub state_store: Arc<StateStore>,
    pub category_dispatcher: Arc<CategoryDispatcher>,
    active_controls: RwLock<HashMap<String, ActiveTaskControl>>,
    pub global_speed_limiter: SpeedLimiter,
    default_connections: usize,
    default_download_dir: PathBuf,
}

impl DownloadEngine {
    pub fn new(
        storage_dir: &Path,
        default_download_dir: PathBuf,
        category_dispatcher: Arc<CategoryDispatcher>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(32)
            .user_agent(DEFAULT_BROWSER_USER_AGENT)
            .build()
            .unwrap_or_default();

        let state_store = Arc::new(StateStore::new(storage_dir));
        let global_speed_limiter = SpeedLimiter::new(0);

        Self {
            client,
            state_store,
            category_dispatcher,
            active_controls: RwLock::new(HashMap::new()),
            global_speed_limiter,
            default_connections: 8,
            default_download_dir,
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Add a new download task and probe remote server headers
    pub async fn add_download(
        &self,
        req: AddDownloadRequest,
        start_immediately: bool,
        app_handle: Option<AppHandle>,
    ) -> Result<DownloadTask, String> {
        let parsed_url = Url::parse(&req.url).map_err(|e| format!("Invalid URL: {}", e))?;

        // 1. Probe headers with HEAD or Range GET
        let (total_size, supports_range, probed_filename) =
            self.probe_url(&req.url, req.headers.as_ref()).await?;

        // Determine filename
        let file_name = if let Some(n) = req.file_name.filter(|s| !s.trim().is_empty()) {
            n
        } else if let Some(pn) = probed_filename {
            pn
        } else {
            parsed_url
                .path_segments()
                .and_then(|mut segs| segs.next_back())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("download.bin")
                .to_string()
        };

        let connections = req.connections.unwrap_or(self.default_connections);
        let category = self.category_dispatcher.classify(&file_name, None);

        // Determine save path using Automatic Category Dispatcher unless explicitly provided
        let save_dir = req
            .save_path
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let routed = self.category_dispatcher.resolve_destination_dir(&category);
                if routed.exists() {
                    routed
                } else {
                    self.default_download_dir.clone()
                }
            });
        let save_file_path = save_dir.join(&file_name);

        let task_id = uuid::Uuid::new_v4().to_string();

        // 2. Initialize segment plan
        let segments = self.calculate_initial_segments(total_size, connections, supports_range);

        let task = DownloadTask {
            id: task_id.clone(),
            url: req.url.clone(),
            file_name,
            save_path: save_file_path.to_string_lossy().to_string(),
            total_size,
            downloaded_bytes: 0,
            speed: 0,
            eta_seconds: None,
            status: if start_immediately {
                DownloadStatus::Connecting
            } else {
                DownloadStatus::Queued
            },
            category,
            supports_range,
            connections: segments.len(),
            segments,
            error_message: None,
            order_index: Utc::now().timestamp_millis(),
            media_info: None,
            headers: req.headers,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.state_store.add_task(task.clone());

        if start_immediately {
            if let Some(app) = app_handle {
                self.start_download(&task_id, app)?;
            }
        }

        Ok(task)
    }

    /// Probes the server using HEAD or Range GET to obtain content length and range capabilities
    async fn probe_url(
        &self,
        url: &str,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<(u64, bool, Option<String>), String> {
        // Try HEAD first
        let head_res = send_request_with_headers(
            &self.client,
            reqwest::Method::HEAD,
            url,
            extra_headers,
            None,
        )
        .await;
        if let Ok(resp) = head_res {
            if resp.status().is_success() {
                let headers = resp.headers();
                let accepts_ranges = headers
                    .get(reqwest::header::ACCEPT_RANGES)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.eq_ignore_ascii_case("bytes"))
                    .unwrap_or(false);

                let content_len = headers
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);

                let filename = headers
                    .get(reqwest::header::CONTENT_DISPOSITION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_disposition_filename);

                if content_len > 0 {
                    return Ok((content_len, accepts_ranges, filename));
                }
            }
        }

        // Fallback: try GET with Range 0-0
        let get_res = send_request_with_headers(
            &self.client,
            reqwest::Method::GET,
            url,
            extra_headers,
            Some("bytes=0-0"),
        )
        .await
        .map_err(|e| format!("Failed to reach host: {}", e))?;

        let status = get_res.status();
        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("Server returned HTTP {}", status));
        }
        let headers = get_res.headers();

        let accepts_ranges = status == reqwest::StatusCode::PARTIAL_CONTENT
            || headers
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.eq_ignore_ascii_case("bytes"))
                .unwrap_or(false);

        let content_len = if let Some(cr) = headers
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
        {
            // e.g. "bytes 0-0/90007055"
            cr.rsplit('/')
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        } else {
            headers
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };

        let filename = headers
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_disposition_filename);

        Ok((content_len, accepts_ranges, filename))
    }

    /// Partitions file range into balanced segments for parallel connections
    fn calculate_initial_segments(
        &self,
        total_size: u64,
        connections: usize,
        supports_range: bool,
    ) -> Vec<SegmentProgress> {
        if !supports_range || total_size < 1024 * 512 || connections <= 1 {
            return vec![SegmentProgress {
                id: 0,
                start_byte: 0,
                end_byte: total_size.saturating_sub(1),
                current_byte: 0,
                downloaded: 0,
                speed: 0,
                status: SegmentStatus::Pending,
            }];
        }

        let num_conns = connections.min(32).max(1);
        let part_size = total_size / (num_conns as u64);
        let mut segments = Vec::with_capacity(num_conns);

        for i in 0..num_conns {
            let start_byte = (i as u64) * part_size;
            let end_byte = if i == num_conns - 1 {
                total_size - 1
            } else {
                ((i + 1) as u64) * part_size - 1
            };

            segments.push(SegmentProgress {
                id: i,
                start_byte,
                end_byte,
                current_byte: start_byte,
                downloaded: 0,
                speed: 0,
                status: SegmentStatus::Pending,
            });
        }

        segments
    }

    /// Starts or resumes a download task
    pub fn start_download(&self, task_id: &str, app_handle: AppHandle) -> Result<(), String> {
        self.start_download_internal(task_id, Some(app_handle))
    }

    /// Starts or resumes a download task cleanly without an active UI window handle (ideal for testing/headless)
    pub fn start_download_headless(&self, task_id: &str) -> Result<(), String> {
        self.start_download_internal(task_id, None)
    }

    fn start_download_internal(&self, task_id: &str, app_handle: Option<AppHandle>) -> Result<(), String> {
        let task = self
            .state_store
            .get_task(task_id)
            .ok_or_else(|| format!("Task {} not found", task_id))?;

        if task.status == DownloadStatus::Completed {
            return Ok(());
        }

        info!(task_id = task_id, file = %task.file_name, connections = task.segments.len(), "Starting download transfer");

        // Cancel previous if running
        self.stop_internal(task_id);

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let speed_limiter = SpeedLimiter::new(self.global_speed_limiter.get_rate());

        {
            let mut controls = self.active_controls.write();
            controls.insert(
                task_id.to_string(),
                ActiveTaskControl {
                    cancel_tx,
                    speed_limiter: speed_limiter.clone(),
                },
            );
        }

        self.state_store
            .update_task_status(task_id, DownloadStatus::Downloading, None);

        // Preallocate target file directly (zero assembly step)
        let save_path = PathBuf::from(&task.save_path);
        let sparse_file = match SparseFile::create_preallocated(&save_path, task.total_size) {
            Ok(f) => f,
            Err(e) => {
                let err_msg = format!("Failed to preallocate file: {}", e);
                error!(task_id = task_id, "{}", err_msg);
                self.state_store
                    .update_task_status(task_id, DownloadStatus::Error, Some(err_msg.clone()));
                return Err(err_msg);
            }
        };

        let file_arc = sparse_file.get_arc_handle();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<WorkerProgressEvent>();

        let initial_downloaded: u64 = task.segments.iter().map(|s| s.downloaded).sum();
        let shared_downloaded_bytes = Arc::new(AtomicU64::new(initial_downloaded));
        let mut seg_handles: Vec<(Arc<AtomicU64>, Arc<AtomicU8>)> =
            Vec::with_capacity(task.segments.len());

        // Spawn chunk workers (workers increment lock-free atomics only; no per-packet IPC spam)
        for seg in &task.segments {
            if seg.status == SegmentStatus::Completed {
                seg_handles.push((
                    Arc::new(AtomicU64::new(seg.current_byte)),
                    Arc::new(AtomicU8::new(SEG_STATE_COMPLETED)),
                ));
                continue;
            }

            let worker = ChunkWorker::new(
                seg.id,
                seg.start_byte,
                seg.end_byte,
                seg.downloaded,
                task.url.clone(),
                self.client.clone(),
                task.headers.clone(),
                Arc::clone(&file_arc),
                Some(speed_limiter.clone()),
                cancel_rx.clone(),
                progress_tx.clone(),
                Arc::clone(&shared_downloaded_bytes),
            );

            seg_handles.push((
                Arc::clone(&worker.current_offset),
                Arc::clone(&worker.segment_state),
            ));

            spawn_bg(async move {
                worker.run().await;
            });
        }

        drop(progress_tx); // Drop original sender so loop terminates when all workers finish

        let store_clone = Arc::clone(&self.state_store);
        let tid = task_id.to_string();
        let total_size = task.total_size;
        let mut segments = task.segments.clone();

        // Dedicated fixed-cadence (500 ms / 2 Hz) telemetry orchestrator with EWMA smoothing & hysteresis
        spawn_bg(async move {
            let mut speed_tracker = EwmaSpeedTracker::new(initial_downloaded);
            speed_tracker.seed_clock(initial_downloaded, Instant::now());

            let start_at = tokio::time::Instant::now() + TELEMETRY_TICK_INTERVAL;
            let mut telemetry_ticker = tokio::time::interval_at(start_at, TELEMETRY_TICK_INTERVAL);
            telemetry_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

            let mut last_error: Option<String> = None;

            loop {
                tokio::select! {
                    biased;

                    maybe_event = progress_rx.recv() => {
                        match maybe_event {
                            Some(event) => {
                                if let Some(err) = event.error {
                                    last_error = Some(err);
                                }
                            }
                            None => break,
                        }
                    }

                    _ = telemetry_ticker.tick() => {
                        let atomic_bytes = shared_downloaded_bytes.load(Ordering::Relaxed);
                        let smoothed_speed = speed_tracker.sample_at(atomic_bytes, Instant::now());

                        let mut downloaded_sum = snapshot_segment_atomics(&mut segments, &seg_handles);
                        if total_size > 0 {
                            downloaded_sum = downloaded_sum.min(total_size);
                        }

                        let eta = speed_tracker.compute_eta(total_size, downloaded_sum);

                        store_clone.update_progress(
                            &tid,
                            downloaded_sum,
                            smoothed_speed,
                            eta,
                            segments.clone(),
                        );

                        if let Some(ref app) = app_handle {
                            let payload = DownloadProgressPayload {
                                id: tid.clone(),
                                status: DownloadStatus::Downloading,
                                downloaded_bytes: downloaded_sum,
                                total_size,
                                speed: smoothed_speed,
                                eta_seconds: eta,
                                segments: segments.clone(),
                                error_message: None,
                                media_info: None,
                            };
                            let _ = app.emit("download-progress", &payload);
                        }
                    }
                }
            }

            // Final state evaluation
            let mut downloaded_sum = snapshot_segment_atomics(&mut segments, &seg_handles);
            if total_size > 0 {
                downloaded_sum = downloaded_sum.min(total_size);
            }
            speed_tracker.reset(downloaded_sum);

            let all_completed = segments.iter().all(|s| s.status == SegmentStatus::Completed)
                || (total_size > 0 && downloaded_sum >= total_size);

            if all_completed {
                let _ = sparse_file.sync_all();
                store_clone.update_progress(&tid, total_size, 0, Some(0), segments.clone());
                store_clone.update_task_status(&tid, DownloadStatus::Completed, None);
                info!(task_id = %tid, bytes = total_size, "Download completed with 0 ms assembly overhead");

                if let Some(ref app) = app_handle {
                    let payload = DownloadProgressPayload {
                        id: tid.clone(),
                        status: DownloadStatus::Completed,
                        downloaded_bytes: total_size,
                        total_size,
                        speed: 0,
                        eta_seconds: Some(0),
                        segments: segments.clone(),
                        error_message: None,
                        media_info: None,
                    };
                    let _ = app.emit("download-progress", &payload);
                }
            } else if let Some(err_msg) = last_error {
                warn!(task_id = %tid, error = %err_msg, "Download stopped with segment error");
                store_clone.update_progress(&tid, downloaded_sum, 0, None, segments.clone());
                store_clone.update_task_status(&tid, DownloadStatus::Error, Some(err_msg.clone()));

                if let Some(ref app) = app_handle {
                    let payload = DownloadProgressPayload {
                        id: tid.clone(),
                        status: DownloadStatus::Error,
                        downloaded_bytes: downloaded_sum,
                        total_size,
                        speed: 0,
                        eta_seconds: None,
                        segments: segments.clone(),
                        error_message: Some(err_msg),
                        media_info: None,
                    };
                    let _ = app.emit("download-progress", &payload);
                }
            } else {
                // If stopped by cancel/pause: reset speed cleanly to 0
                store_clone.update_progress(&tid, downloaded_sum, 0, None, segments.clone());
                let current_task = store_clone.get_task(&tid);
                if let Some(t) = current_task {
                    if t.status == DownloadStatus::Downloading {
                        store_clone.update_task_status(&tid, DownloadStatus::Paused, None);
                    }
                    if let Some(ref app) = app_handle {
                        let payload = DownloadProgressPayload {
                            id: tid.clone(),
                            status: DownloadStatus::Paused,
                            downloaded_bytes: downloaded_sum,
                            total_size,
                            speed: 0,
                            eta_seconds: None,
                            segments: segments.clone(),
                            error_message: None,
                            media_info: None,
                        };
                        let _ = app.emit("download-progress", &payload);
                    }
                }
            }
        });

        Ok(())
    }

    /// Registers an external background worker cancellation token (e.g. OctoMedia / yt-dlp)
    /// so UI Pause / Delete actions abort the subprocess tree cleanly.
    pub fn register_external_cancel(&self, task_id: &str) -> watch::Receiver<bool> {
        self.stop_internal(task_id);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let speed_limiter = SpeedLimiter::new(self.global_speed_limiter.get_rate());
        let mut controls = self.active_controls.write();
        controls.insert(
            task_id.to_string(),
            ActiveTaskControl {
                cancel_tx,
                speed_limiter,
            },
        );
        cancel_rx
    }

    pub fn unregister_external_control(&self, task_id: &str) {
        self.active_controls.write().remove(task_id);
    }

    /// Pause an active download
    pub fn pause_download(&self, task_id: &str) -> Result<(), String> {
        self.stop_internal(task_id);
        self.state_store
            .update_task_status(task_id, DownloadStatus::Paused, None);
        Ok(())
    }

    /// Stop task and remove active controller
    fn stop_internal(&self, task_id: &str) {
        let mut controls = self.active_controls.write();
        if let Some(ctrl) = controls.remove(task_id) {
            let _ = ctrl.cancel_tx.send(true);
        }
    }

    /// Set dynamic speed limit for a specific task or globally (0 = unlimited)
    pub fn set_speed_limit(&self, task_id: Option<&str>, limit_bytes_per_sec: u64) {
        if let Some(id) = task_id {
            let controls = self.active_controls.read();
            if let Some(ctrl) = controls.get(id) {
                ctrl.speed_limiter.set_rate(limit_bytes_per_sec);
            }
        } else {
            self.global_speed_limiter.set_rate(limit_bytes_per_sec);
            let controls = self.active_controls.read();
            for ctrl in controls.values() {
                ctrl.speed_limiter.set_rate(limit_bytes_per_sec);
            }
        }
    }

    /// Delete a task and optionally delete the file from disk
    pub fn delete_task(&self, task_id: &str, delete_file: bool) -> Result<(), String> {
        self.stop_internal(task_id);
        self.state_store.delete_task(task_id, delete_file);
        Ok(())
    }
}

fn snapshot_segment_atomics(
    segments: &mut [SegmentProgress],
    handles: &[(Arc<AtomicU64>, Arc<AtomicU8>)],
) -> u64 {
    let mut sum = 0u64;
    for (seg, (offset_arc, state_arc)) in segments.iter_mut().zip(handles.iter()) {
        let cur = offset_arc.load(Ordering::Acquire);
        seg.current_byte = cur;
        seg.downloaded = cur.saturating_sub(seg.start_byte);
        seg.status = match state_arc.load(Ordering::Acquire) {
            SEG_STATE_DOWNLOADING => SegmentStatus::Downloading,
            SEG_STATE_COMPLETED => SegmentStatus::Completed,
            SEG_STATE_FAILED => SegmentStatus::Failed,
            SEG_STATE_PENDING => SegmentStatus::Pending,
            _ => seg.status.clone(),
        };
        sum = sum.saturating_add(seg.downloaded);
    }
    sum
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

fn parse_content_disposition_filename(cd: &str) -> Option<String> {
    for part in cd.split(';') {
        let part = part.trim();
        if part.to_ascii_lowercase().starts_with("filename*=") {
            if let Some(val) = part.split('=').nth(1) {
                let clean = val.trim_matches(|c| c == '"' || c == '\'');
                if let Some(decoded) = clean.split("''").nth(1) {
                    return Some(decoded.to_string());
                }
                return Some(clean.to_string());
            }
        } else if part.to_ascii_lowercase().starts_with("filename=") {
            if let Some(val) = part.split('=').nth(1) {
                let clean = val.trim_matches(|c| c == '"' || c == '\'');
                return Some(clean.to_string());
            }
        }
    }
    None
}
