use crate::engine::batch::expand_pattern;
use crate::engine::categories::{CategoryDispatcher, CategoryRoutes};
use crate::engine::logging::{self, LogEntry};
use crate::engine::media_muxer::{MediaManifestInfo, MediaMuxer};
use crate::engine::native_host::dispatch_split_media_task;
use crate::engine::omniget::downloader::{resume_omniget_task, start_omniget_download};
use crate::engine::omniget::extractor::fetch_media_formats;
use crate::engine::omniget::process::{ensure_tools_installed, ResolvedTools};
use crate::engine::omniget::{
    OmnigetDownloadRequest, OmnigetFetchRequest, OmnigetMediaInfo, OmnigetToolsStatus,
    PendingVideoGrab, VideoGrabberPayload,
};
use crate::engine::post_process::{AntivirusScanResult, HashVerificationResult, PostProcessor};
use crate::engine::proxy_auth::{ProxyAuthManager, ProxyAuthSettings};
use crate::engine::scheduler::{QueueConfig, QueueScheduler};
use crate::engine::settings::{
    detect_system_locale as detect_locale_impl, write_registry_language, AppSettings,
    SettingsStore, DEFAULT_INTERCEPT_EXTENSIONS,
};
use crate::engine::types::{
    AddDownloadRequest, DownloadCategory, DownloadProgressPayload, DownloadStatus, DownloadTask,
    MediaStreamInfo, SegmentProgress, SegmentStatus,
};
use crate::engine::zip_preview::{RemoteZipPreviewer, ZipFileEntry, ZipPreviewResult};
use crate::engine::{drag_drop, ClipboardMonitor, DownloadEngine};
use crate::shell_integration::{BrowserIntegrationReport, ShellIntegration, ShellIntegrationStatus};
use crate::RuntimePaths;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub type SharedEngine = Arc<DownloadEngine>;
pub type SharedScheduler = Arc<QueueScheduler>;
pub type SharedProxyAuth = Arc<ProxyAuthManager>;
pub type SharedCategoryDispatcher = Arc<CategoryDispatcher>;
pub type SharedSettingsStore = Arc<SettingsStore>;
pub type SharedPendingGrab = Arc<PendingVideoGrab>;
pub type SharedClipboardMonitor = Arc<ClipboardMonitor>;

// --- Download Tasks ---

#[tauri::command]
pub fn get_tasks(engine: State<SharedEngine>) -> Vec<DownloadTask> {
    engine.state_store.get_all_tasks()
}

#[tauri::command]
pub fn get_task(id: String, engine: State<SharedEngine>) -> Option<DownloadTask> {
    engine.state_store.get_task(&id)
}

#[tauri::command]
pub async fn add_download(
    request: AddDownloadRequest,
    start_immediately: bool,
    engine: State<'_, SharedEngine>,
    app_handle: AppHandle,
) -> Result<DownloadTask, String> {
    engine
        .add_download(request, start_immediately, Some(app_handle))
        .await
}

fn start_or_resume_task_impl(
    id: &str,
    engine: &SharedEngine,
    app_handle: AppHandle,
) -> Result<(), String> {
    if let Some(task) = engine.state_store.get_task(id) {
        if task
            .media_info
            .as_ref()
            .map(|mi| mi.protocol == "YT-DLP")
            .unwrap_or(false)
        {
            return resume_omniget_task(id, Arc::clone(engine), Some(app_handle));
        }
    }
    engine.start_download(id, app_handle)
}

#[tauri::command]
pub fn start_download(
    id: String,
    engine: State<SharedEngine>,
    app_handle: AppHandle,
) -> Result<(), String> {
    start_or_resume_task_impl(&id, engine.inner(), app_handle)
}

#[tauri::command]
pub fn pause_download(id: String, engine: State<SharedEngine>) -> Result<(), String> {
    engine.pause_download(&id)
}

#[tauri::command]
pub fn resume_download(
    id: String,
    engine: State<SharedEngine>,
    app_handle: AppHandle,
) -> Result<(), String> {
    start_or_resume_task_impl(&id, engine.inner(), app_handle)
}

#[tauri::command]
pub fn delete_download(
    id: String,
    delete_file: bool,
    engine: State<SharedEngine>,
) -> Result<(), String> {
    engine.delete_task(&id, delete_file)
}

// --- Rate Limiting ---

#[tauri::command]
pub fn set_speed_limit(
    id: Option<String>,
    limit_bytes_per_sec: u64,
    engine: State<SharedEngine>,
) -> Result<(), String> {
    engine.set_speed_limit(id.as_deref(), limit_bytes_per_sec);
    Ok(())
}

#[tauri::command]
pub fn get_speed_limit(engine: State<SharedEngine>) -> u64 {
    engine.global_speed_limiter.get_rate()
}

// --- Scheduler & Queues ---

#[tauri::command]
pub fn get_queues(scheduler: State<SharedScheduler>) -> Vec<QueueConfig> {
    scheduler.get_all_queues()
}

#[tauri::command]
pub fn save_queue(config: QueueConfig, scheduler: State<SharedScheduler>) -> Result<(), String> {
    scheduler.save_queue(config);
    Ok(())
}

#[tauri::command]
pub fn delete_queue(id: String, scheduler: State<SharedScheduler>) -> Result<(), String> {
    if scheduler.delete_queue(&id) {
        Ok(())
    } else {
        Err(format!("Queue {} not found", id))
    }
}

#[tauri::command]
pub fn add_task_to_queue(
    queue_id: String,
    task_id: String,
    scheduler: State<SharedScheduler>,
) -> Result<(), String> {
    scheduler.add_task_to_queue(&queue_id, &task_id);
    Ok(())
}

#[tauri::command]
pub fn remove_task_from_queue(
    queue_id: String,
    task_id: String,
    scheduler: State<SharedScheduler>,
) -> Result<(), String> {
    scheduler.remove_task_from_queue(&queue_id, &task_id);
    Ok(())
}

// --- Batch Downloads ---

#[tauri::command]
pub fn expand_batch_pattern(pattern: String) -> Result<Vec<String>, String> {
    expand_pattern(&pattern)
}

#[tauri::command]
pub async fn add_batch_downloads(
    urls: Vec<String>,
    queue_id: Option<String>,
    start_immediately: bool,
    engine: State<'_, SharedEngine>,
    scheduler: State<'_, SharedScheduler>,
    app_handle: AppHandle,
) -> Result<Vec<DownloadTask>, String> {
    let mut tasks = Vec::new();

    for url in urls {
        let req = AddDownloadRequest {
            url,
            save_path: None,
            file_name: None,
            connections: None,
            headers: None,
        };

        let start_this = start_immediately && queue_id.is_none();
        if let Ok(task) = engine
            .add_download(req, start_this, Some(app_handle.clone()))
            .await
        {
            if let Some(ref q_id) = queue_id {
                scheduler.add_task_to_queue(q_id, &task.id);
            }
            tasks.push(task);
        }
    }

    Ok(tasks)
}

// --- Remote ZIP Preview & Partial Extraction ---

#[tauri::command]
pub async fn preview_remote_zip(
    url: String,
    proxy_auth: State<'_, SharedProxyAuth>,
) -> Result<ZipPreviewResult, String> {
    let client = proxy_auth.build_client();
    RemoteZipPreviewer::preview_remote_zip(&client, &url).await
}

#[tauri::command]
pub async fn download_zip_entry(
    url: String,
    entry: ZipFileEntry,
    save_path: Option<String>,
    proxy_auth: State<'_, SharedProxyAuth>,
    dispatcher: State<'_, SharedCategoryDispatcher>,
) -> Result<String, String> {
    let client = proxy_auth.build_client();
    let target_path = if let Some(p) = save_path {
        PathBuf::from(p)
    } else {
        let downloads = PathBuf::from(dispatcher.get_routes().base_download_dir);
        let safe_name = Path::new(&entry.name)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "extracted_file.bin".to_string());
        downloads.join(safe_name)
    };

    RemoteZipPreviewer::extract_single_entry(&client, &url, &entry, &target_path).await?;
    Ok(target_path.to_string_lossy().to_string())
}

// --- Proxy & Authentication Settings ---

#[tauri::command]
pub fn get_proxy_auth_settings(proxy_auth: State<SharedProxyAuth>) -> ProxyAuthSettings {
    proxy_auth.get_settings()
}

#[tauri::command]
pub fn save_proxy_auth_settings(
    settings: ProxyAuthSettings,
    proxy_auth: State<SharedProxyAuth>,
) -> Result<(), String> {
    proxy_auth.update_settings(settings);
    Ok(())
}

// --- Integrity Check & Antivirus ---

#[tauri::command]
pub fn verify_file_hash(
    path: String,
    expected_hash: Option<String>,
) -> Result<HashVerificationResult, String> {
    PostProcessor::compute_file_hashes(Path::new(&path), expected_hash.as_deref())
}

#[tauri::command]
pub async fn scan_file_antivirus(
    path: String,
    custom_scanner: Option<String>,
    custom_args: Option<String>,
) -> Result<AntivirusScanResult, String> {
    PostProcessor::scan_file(
        Path::new(&path),
        custom_scanner.as_deref(),
        custom_args.as_deref(),
    )
    .await
}

// --- System & Shell Helpers ---

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let p = std::path::Path::new(&path);
        let folder = if p.is_file() {
            p.parent().unwrap_or(p)
        } else {
            p
        };
        std::process::Command::new("explorer")
            .arg(folder)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- Queue Priority Reordering ---

#[tauri::command]
pub fn reorder_task(
    id: String,
    new_index: usize,
    engine: State<SharedEngine>,
    scheduler: State<SharedScheduler>,
) -> Result<(), String> {
    engine.state_store.reorder_task(&id, new_index)?;
    scheduler.sync_task_order_across_queues(&id, true);
    Ok(())
}

#[tauri::command]
pub fn move_task_up(
    id: String,
    engine: State<SharedEngine>,
    scheduler: State<SharedScheduler>,
) -> Result<(), String> {
    engine.state_store.move_task_up(&id)?;
    scheduler.sync_task_order_across_queues(&id, true);
    Ok(())
}

#[tauri::command]
pub fn move_task_down(
    id: String,
    engine: State<SharedEngine>,
    scheduler: State<SharedScheduler>,
) -> Result<(), String> {
    engine.state_store.move_task_down(&id)?;
    scheduler.sync_task_order_across_queues(&id, false);
    Ok(())
}

// --- Category Folder Routing ---

#[tauri::command]
pub fn get_category_routes(
    dispatcher: State<SharedCategoryDispatcher>,
) -> CategoryRoutes {
    dispatcher.get_routes()
}

#[tauri::command]
pub fn set_category_route(
    category: DownloadCategory,
    custom_path: Option<String>,
    dispatcher: State<SharedCategoryDispatcher>,
) -> Result<(), String> {
    dispatcher.update_route(category, custom_path)
}

#[tauri::command]
pub fn reset_category_routes(
    dispatcher: State<SharedCategoryDispatcher>,
) -> Result<(), String> {
    dispatcher.reset_routes();
    Ok(())
}

// --- Windows Shell Integration & Auto-Start ---

#[tauri::command]
pub fn get_shell_integration_status() -> ShellIntegrationStatus {
    ShellIntegration::get_status()
}

#[tauri::command]
pub fn set_autostart(enable: bool, app: AppHandle) -> Result<(), String> {
    ShellIntegration::set_autostart(enable)?;
    crate::tray::sync_tray_autostart_checked(&app, enable);
    Ok(())
}

#[tauri::command]
pub fn set_uri_scheme(enable: bool) -> Result<(), String> {
    ShellIntegration::set_uri_scheme(enable)
}

#[tauri::command]
pub fn set_context_menu(enable: bool) -> Result<(), String> {
    ShellIntegration::set_context_menu(enable)
}

// --- HLS / DASH Media Stream Inspection & Native Remuxing ---

#[tauri::command]
pub async fn inspect_media_stream(
    url: String,
    engine: State<'_, SharedEngine>,
) -> Result<MediaManifestInfo, String> {
    MediaMuxer::inspect_manifest(engine.client(), &url).await
}

#[tauri::command]
pub async fn start_media_stream_download(
    url: String,
    save_path: Option<String>,
    engine: State<'_, SharedEngine>,
    dispatcher: State<'_, SharedCategoryDispatcher>,
    app: AppHandle,
) -> Result<DownloadTask, String> {
    let manifest = MediaMuxer::inspect_manifest(engine.client(), &url).await?;
    let vid = manifest.video_tracks.first().cloned();
    let res = manifest.recommended_video_resolution.clone();
    let audio_br = manifest.recommended_audio_bitrate;

    let file_name = format!("stream_{}.mp4", Utc::now().format("%Y%m%d_%H%M%S"));
    let target_dir = save_path
        .map(PathBuf::from)
        .unwrap_or_else(|| dispatcher.resolve_destination_dir(&DownloadCategory::Video));
    let dest_file = target_dir.join(&file_name);

    let task_id = uuid::Uuid::new_v4().to_string();
    let total_segs = vid.as_ref().map(|v| v.segment_urls.len()).unwrap_or(0);

    let media_info = MediaStreamInfo {
        protocol: manifest.protocol.clone(),
        video_resolution: res,
        video_codec: vid.as_ref().map(|v| v.codec.clone()),
        audio_codec: Some("mp4a".to_string()),
        audio_bitrate: audio_br,
        total_segments: total_segs,
        completed_segments: 0,
        mux_phase: "downloading".to_string(),
        mux_percent: 0.0,
        ..Default::default()
    };

    let task = DownloadTask {
        id: task_id.clone(),
        url: url.clone(),
        file_name: file_name.clone(),
        save_path: dest_file.to_string_lossy().to_string(),
        total_size: (total_segs as u64) * 1_500_000,
        downloaded_bytes: 0,
        speed: 0,
        eta_seconds: None,
        status: DownloadStatus::Downloading,
        category: DownloadCategory::Video,
        supports_range: true,
        connections: 6,
        segments: vec![SegmentProgress {
            id: 0,
            start_byte: 0,
            end_byte: (total_segs as u64) * 1_500_000,
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

    let tid = task_id.clone();
    let client = engine.client().clone();
    let store = engine.state_store.clone();
    let app_clone = app.clone();

    tauri::async_runtime::spawn(async move {
        let tid_c = tid.clone();
        let app_c = app_clone.clone();
        let mut current_info = media_info;

        let res = MediaMuxer::download_and_remux(
            &client,
            &url,
            &dest_file,
            move |prog| {
                current_info.mux_phase = prog.phase.clone();
                current_info.completed_segments = prog.segments_done;
                current_info.total_segments = prog.segments_total;
                current_info.mux_percent = prog.percent;

                let payload = DownloadProgressPayload {
                    id: tid_c.clone(),
                    status: if prog.phase == "completed" {
                        DownloadStatus::Completed
                    } else {
                        DownloadStatus::Downloading
                    },
                    downloaded_bytes: prog.downloaded_bytes,
                    total_size: prog.total_bytes,
                    speed: prog.speed_bytes_per_sec,
                    eta_seconds: prog.eta_seconds,
                    segments: prog.active_segments,
                    error_message: None,
                    media_info: Some(current_info.clone()),
                };
                let _ = app_c.emit("download-progress", &payload);
            },
        ).await;

        match res {
            Ok(_) => {
                store.update_task_status(&tid, DownloadStatus::Completed, None);
            }
            Err(e) => {
                store.update_task_status(&tid, DownloadStatus::Error, Some(e));
            }
        }
    });

    Ok(task)
}

// --- Diagnostics & Structured Logging ---

#[tauri::command]
pub fn get_diagnostic_logs() -> Vec<LogEntry> {
    logging::get_logs()
}

#[tauri::command]
pub fn clear_diagnostic_logs() {
    logging::clear_logs();
}

#[tauri::command]
pub fn export_diagnostic_logs() -> String {
    logging::export_logs_text()
}

// --- Queue & Task State Export / Import ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OctoHeftStateBackup {
    pub version: u32,
    pub exported_at: String,
    pub tasks: Vec<DownloadTask>,
    pub queues: Vec<QueueConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportStateResult {
    pub tasks_imported: usize,
    pub queues_imported: usize,
}

#[tauri::command]
pub fn export_state_json(
    engine: State<SharedEngine>,
    scheduler: State<SharedScheduler>,
) -> Result<String, String> {
    let backup = OctoHeftStateBackup {
        version: 1,
        exported_at: Utc::now().to_rfc3339(),
        tasks: engine.state_store.get_all_tasks(),
        queues: scheduler.get_all_queues(),
    };

    tracing::info!(
        tasks = backup.tasks.len(),
        queues = backup.queues.len(),
        "Exported OctoHeft task and queue state"
    );

    serde_json::to_string_pretty(&backup).map_err(|e| format!("Serialization failed: {}", e))
}

#[tauri::command]
pub fn import_state_json(
    json: String,
    engine: State<SharedEngine>,
    scheduler: State<SharedScheduler>,
) -> Result<ImportStateResult, String> {
    if let Ok(backup) = serde_json::from_str::<OctoHeftStateBackup>(&json) {
        let tasks_imported = engine.state_store.import_tasks(backup.tasks);
        let queues_imported = scheduler.import_queues(backup.queues);
        tracing::info!(
            tasks_imported = tasks_imported,
            queues_imported = queues_imported,
            "Imported OctoHeft backup bundle"
        );
        return Ok(ImportStateResult {
            tasks_imported,
            queues_imported,
        });
    }

    // Fallback: allow importing a raw array of DownloadTask entries
    if let Ok(raw_tasks) = serde_json::from_str::<Vec<DownloadTask>>(&json) {
        let tasks_imported = engine.state_store.import_tasks(raw_tasks);
        tracing::info!(tasks_imported = tasks_imported, "Imported raw task list");
        return Ok(ImportStateResult {
            tasks_imported,
            queues_imported: 0,
        });
    }

    Err("Invalid OctoHeft JSON backup format".to_string())
}

// --- Application Settings & Localization ---

#[tauri::command]
pub fn get_app_settings(settings_store: State<SharedSettingsStore>) -> AppSettings {
    settings_store.load_or_init()
}

#[tauri::command]
pub fn get_default_intercept_extensions() -> String {
    DEFAULT_INTERCEPT_EXTENSIONS.to_string()
}

#[tauri::command]
pub fn save_app_settings(
    settings: AppSettings,
    settings_store: State<SharedSettingsStore>,
    dispatcher: State<SharedCategoryDispatcher>,
    monitor: State<SharedClipboardMonitor>,
    engine: State<SharedEngine>,
    app: AppHandle,
) -> Result<AppSettings, String> {
    let saved = settings_store.save(&settings)?;
    engine.set_default_connections(saved.max_connections_per_download as usize);
    write_registry_language(&saved.language);
    crate::tray::update_tray_language(&app, &saved.language);
    dispatcher.apply_folder_settings(
        PathBuf::from(&saved.default_download_dir),
        saved.use_category_subfolders,
    );
    if saved.clipboard_monitor_enabled != monitor.is_active() {
        monitor.set_active(saved.clipboard_monitor_enabled);
    }
    let _ = app.emit("settings-updated", &saved);
    Ok(saved)
}

#[tauri::command]
pub fn set_app_language(
    language: String,
    settings_store: State<SharedSettingsStore>,
    app: AppHandle,
) -> Result<AppSettings, String> {
    let saved = settings_store.set_language(&language)?;
    write_registry_language(&saved.language);
    crate::tray::update_tray_language(&app, &saved.language);
    let _ = app.emit("settings-updated", &saved);
    Ok(saved)
}

#[tauri::command]
pub async fn pick_download_folder(title: Option<String>) -> Result<Option<String>, String> {
    let prompt = title.unwrap_or_else(|| "Select default download folder".to_string());
    tokio::task::spawn_blocking(move || ShellIntegration::pick_folder_dialog(&prompt))
        .await
        .map_err(|e| format!("Folder dialog join error: {}", e))?
}

#[tauri::command]
pub fn detect_system_locale() -> String {
    detect_locale_impl()
}

#[tauri::command]
pub async fn start_split_media_download(
    video_url: String,
    audio_url: Option<String>,
    file_name: Option<String>,
    save_path: Option<String>,
    headers: Option<std::collections::HashMap<String, String>>,
    engine: State<'_, SharedEngine>,
    app: AppHandle,
) -> Result<DownloadTask, String> {
    if !video_url.starts_with("http://") && !video_url.starts_with("https://") {
        return Err("Split media download requires HTTP/HTTPS stream URLs".to_string());
    }
    Ok(dispatch_split_media_task(
        video_url,
        audio_url,
        file_name,
        save_path.map(PathBuf::from),
        headers,
        None,
        None,
        engine.inner().clone(),
        Some(app),
    )
    .await)
}

// --- Clipboard URL Sniffer & Passive Monitor ---

fn apply_clipboard_monitor_toggle(
    enable: bool,
    monitor: &State<SharedClipboardMonitor>,
    settings_store: &State<SharedSettingsStore>,
    app: &AppHandle,
) -> bool {
    let active = monitor.set_active(enable);
    match settings_store.set_clipboard_monitor_enabled(active) {
        Ok(saved) => {
            let _ = app.emit("settings-updated", &saved);
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed persisting clipboard_monitor_enabled setting");
        }
    }
    tracing::info!(
        clipboard_monitor_enabled = active,
        "Clipboard monitor toggled"
    );
    active
}

#[tauri::command]
pub fn get_clipboard_url() -> Option<String> {
    ShellIntegration::read_clipboard_url()
}

#[tauri::command]
pub fn toggle_clipboard_monitor(
    enable: bool,
    monitor: State<SharedClipboardMonitor>,
    settings_store: State<SharedSettingsStore>,
    app: AppHandle,
) -> bool {
    apply_clipboard_monitor_toggle(enable, &monitor, &settings_store, &app)
}

#[tauri::command]
pub fn get_clipboard_monitor_status(monitor: State<SharedClipboardMonitor>) -> bool {
    monitor.is_active()
}

#[tauri::command]
pub fn set_clipboard_monitor_active(
    enabled: bool,
    monitor: State<SharedClipboardMonitor>,
    settings_store: State<SharedSettingsStore>,
    app: AppHandle,
) -> bool {
    apply_clipboard_monitor_toggle(enabled, &monitor, &settings_store, &app)
}

#[tauri::command]
pub fn get_clipboard_monitor_state(monitor: State<SharedClipboardMonitor>) -> bool {
    monitor.is_active()
}

// --- Outbound Windows OLE/Shell Drag & Drop ---

#[tauri::command]
pub async fn start_file_drag(file_path: String, app_handle: AppHandle) -> Result<(), String> {
    drag_drop::start_file_drag(&app_handle, file_path).await
}

// --- Browser Extension & Native Host Integration ---

#[tauri::command]
pub fn get_browser_integration_status(paths: State<RuntimePaths>) -> BrowserIntegrationReport {
    ShellIntegration::get_browser_integration_report(&paths.storage_dir)
}

#[tauri::command]
pub fn repair_browser_integration(
    paths: State<RuntimePaths>,
) -> Result<BrowserIntegrationReport, String> {
    ShellIntegration::repair_browser_integration(&paths.storage_dir)
}

#[tauri::command]
pub fn open_browser_extensions_page(
    browser_id: Option<String>,
    paths: State<RuntimePaths>,
) -> Result<(), String> {
    ShellIntegration::open_browser_extensions_page(&paths.storage_dir, browser_id.as_deref())
}

#[tauri::command]
pub fn open_extension_folder() -> Result<(), String> {
    let dir = ShellIntegration::resolve_extension_dir();
    if !dir.exists() {
        return Err(format!(
            "Extension folder not found at {}",
            dir.to_string_lossy()
        ));
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn copy_text_to_clipboard(text: String) -> Result<(), String> {
    ShellIntegration::write_clipboard_text(&text)
}

#[tauri::command]
pub fn dismiss_browser_onboarding(
    completed: bool,
    paths: State<RuntimePaths>,
) -> Result<(), String> {
    ShellIntegration::set_onboarding_completed(&paths.storage_dir, completed)
}

// --- OctoMedia (Omniget / yt-dlp + FFmpeg) Engine ---

#[tauri::command]
pub async fn omniget_check_tools() -> Result<OmnigetToolsStatus, String> {
    tokio::task::spawn_blocking(|| ResolvedTools::discover().to_status(true))
        .await
        .map_err(|e| format!("Failed checking OctoMedia tools: {}", e))
}

#[tauri::command]
pub async fn omniget_ensure_tools() -> Result<OmnigetToolsStatus, String> {
    ensure_tools_installed().await
}

#[tauri::command]
pub async fn omniget_fetch_formats(request: OmnigetFetchRequest) -> Result<OmnigetMediaInfo, String> {
    fetch_media_formats(&request).await
}

#[tauri::command]
pub async fn omniget_start_download(
    request: OmnigetDownloadRequest,
    engine: State<'_, SharedEngine>,
    app_handle: AppHandle,
) -> Result<DownloadTask, String> {
    start_omniget_download(request, Arc::clone(engine.inner()), Some(app_handle))
}

#[tauri::command]
pub fn take_pending_video_grab(pending: State<SharedPendingGrab>) -> Option<VideoGrabberPayload> {
    pending.take()
}

// --- System Storage & Disk Space Telemetry ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSpaceStats {
    pub drive_label: String,
    pub target_path: String,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
}

fn extract_drive_label(path: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        for comp in path.components() {
            if let std::path::Component::Prefix(prefix_comp) = comp {
                match prefix_comp.kind() {
                    std::path::Prefix::Disk(letter) | std::path::Prefix::VerbatimDisk(letter) => {
                        return format!("{}:", (letter as char).to_ascii_uppercase());
                    }
                    std::path::Prefix::UNC(server, share)
                    | std::path::Prefix::VerbatimUNC(server, share) => {
                        let s = server.to_string_lossy();
                        let sh = share.to_string_lossy();
                        return if sh.is_empty() {
                            s.to_string()
                        } else {
                            format!("{}\\{}", s, sh)
                        };
                    }
                    _ => {}
                }
            }
        }
    }
    "/".to_string()
}

pub fn query_disk_space_stats(target_path: &Path) -> Result<DiskSpaceStats, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let drive_label = extract_drive_label(target_path);
        let mut probe: PathBuf = target_path.to_path_buf();
        while !probe.exists() {
            match probe.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => probe = parent.to_path_buf(),
                _ => break,
            }
        }
        if !probe.exists() && drive_label.ends_with(':') {
            probe = PathBuf::from(format!("{}\\", drive_label));
        }

        let wide: Vec<u16> = probe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut avail: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;

        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut total_free) };
        if ok == 0 {
            return Err(format!(
                "GetDiskFreeSpaceExW failed for '{}' ({})",
                probe.display(),
                std::io::Error::last_os_error()
            ));
        }

        let used_bytes = total.saturating_sub(avail);
        let used_percent = if total > 0 {
            ((used_bytes as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        return Ok(DiskSpaceStats {
            drive_label,
            target_path: target_path.to_string_lossy().to_string(),
            available_bytes: avail,
            total_bytes: total,
            used_bytes,
            used_percent,
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = target_path;
        Err("Disk space query only implemented for Windows".to_string())
    }
}

#[tauri::command]
pub fn get_disk_space_stats(
    target_dir: Option<String>,
    paths: State<RuntimePaths>,
    settings_store: State<SharedSettingsStore>,
    dispatcher: State<SharedCategoryDispatcher>,
) -> Result<DiskSpaceStats, String> {
    let resolved_path = if let Some(dir) = target_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        PathBuf::from(dir)
    } else {
        let settings = settings_store.load_or_init();
        let configured = settings.default_download_dir.trim();
        if !configured.is_empty() {
            PathBuf::from(configured)
        } else {
            let route_base = dispatcher.get_routes().base_download_dir;
            if !route_base.trim().is_empty() {
                PathBuf::from(route_base)
            } else {
                paths.downloads_dir.clone()
            }
        }
    };

    query_disk_space_stats(&resolved_path)
}

// --- In-App Updates ---

#[tauri::command]
pub async fn check_app_update() -> Result<crate::engine::updater::UpdateInfo, String> {
    crate::engine::updater::check_update(false).await
}

#[tauri::command]
pub async fn install_app_update(download_url: String, app: AppHandle) -> Result<(), String> {
    crate::engine::updater::install_update(download_url, app).await
}

#[tauri::command]
pub fn launch_app_update(app: AppHandle) -> Result<(), String> {
    crate::engine::updater::launch_update(app)
}

#[cfg(test)]
mod disk_stats_tests {
    use super::*;

    #[test]
    fn test_query_disk_space_stats_on_windows() {
        let temp_dir = std::env::temp_dir().join("octoheft_nonexistent_subdir_for_disk_probe");
        let stats = query_disk_space_stats(&temp_dir).expect("disk space query should succeed");
        assert!(stats.total_bytes > 0);
        assert!(stats.available_bytes <= stats.total_bytes);
        assert!(stats.drive_label.ends_with(':') || !stats.drive_label.is_empty());
        assert!(stats.used_percent >= 0.0 && stats.used_percent <= 100.0);
    }
}
