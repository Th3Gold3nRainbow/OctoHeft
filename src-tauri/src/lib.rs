pub mod commands;
pub mod engine;
pub mod shell_integration;
pub mod tray;

use engine::{
    CategoryDispatcher, ClipboardMonitor, DownloadEngine, ProxyAuthManager, QueueScheduler,
    SettingsStore,
};
use shell_integration::ShellIntegration;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

#[derive(Clone)]
pub struct RuntimePaths {
    pub storage_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub portable_mode: bool,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (storage_dir, logs_dir, downloads_dir, is_portable) = resolve_runtime_paths();
    let _ = std::fs::create_dir_all(&storage_dir);
    let _ = std::fs::create_dir_all(&logs_dir);

    let runtime_paths = RuntimePaths {
        storage_dir: storage_dir.clone(),
        logs_dir: logs_dir.clone(),
        downloads_dir: downloads_dir.clone(),
        portable_mode: is_portable,
    };

    engine::logging::init_logging(&logs_dir);
    tracing::info!(
        portable_mode = is_portable,
        storage_dir = %storage_dir.display(),
        downloads_dir = %downloads_dir.display(),
        "OctoHeft core engine initializing"
    );

    let settings_store = Arc::new(SettingsStore::new(&storage_dir));
    let initial_settings = settings_store.load_or_init();
    let effective_downloads_dir = {
        let trimmed = initial_settings.default_download_dir.trim();
        if trimmed.is_empty() {
            downloads_dir.clone()
        } else {
            PathBuf::from(trimmed)
        }
    };

    let category_dispatcher = Arc::new(CategoryDispatcher::new(
        &storage_dir,
        effective_downloads_dir.clone(),
    ));
    category_dispatcher.apply_folder_settings(
        effective_downloads_dir.clone(),
        initial_settings.use_category_subfolders,
    );
    let engine = Arc::new(DownloadEngine::new(
        &storage_dir,
        effective_downloads_dir,
        category_dispatcher.clone(),
    ));
    engine.set_default_connections(initial_settings.max_connections_per_download as usize);
    let scheduler = Arc::new(QueueScheduler::new(&storage_dir));
    let proxy_auth = Arc::new(ProxyAuthManager::new(&storage_dir));
    let pending_grab = Arc::new(engine::omniget::PendingVideoGrab::default());
    let clipboard_monitor = Arc::new(ClipboardMonitor::new());
    clipboard_monitor.set_active(initial_settings.clipboard_monitor_enabled);

    let engine_for_setup = engine.clone();
    let engine_for_pipe = engine.clone();
    let settings_for_pipe = settings_store.clone();
    let settings_for_clipboard = settings_store.clone();
    let pending_for_pipe = pending_grab.clone();
    let scheduler_for_setup = scheduler.clone();
    let clipboard_for_setup = clipboard_monitor.clone();
    let initial_lang_for_tray = initial_settings.language.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .manage(runtime_paths)
        .manage(engine)
        .manage(scheduler)
        .manage(proxy_auth)
        .manage(category_dispatcher)
        .manage(settings_store)
        .manage(pending_grab)
        .manage(clipboard_monitor)
        .setup(move |app| {
            let _ = tray::setup_tray(app.handle(), &initial_lang_for_tray);
            scheduler_for_setup.start_scheduler_loop(engine_for_setup, app.handle().clone());
            engine::native_host::start_native_ipc_server(
                engine_for_pipe,
                settings_for_pipe,
                pending_for_pipe,
                app.handle().clone(),
            );
            clipboard_for_setup.start_watcher(settings_for_clipboard, app.handle().clone());
            ShellIntegration::start_global_hotkey_listener(app.handle().clone());

            if std::env::args().any(|a| a == "--minimized" || a == "--autostart") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_tasks,
            commands::get_task,
            commands::add_download,
            commands::start_download,
            commands::pause_download,
            commands::resume_download,
            commands::delete_download,
            commands::set_speed_limit,
            commands::get_speed_limit,
            commands::get_queues,
            commands::save_queue,
            commands::delete_queue,
            commands::add_task_to_queue,
            commands::remove_task_from_queue,
            commands::expand_batch_pattern,
            commands::add_batch_downloads,
            commands::preview_remote_zip,
            commands::download_zip_entry,
            commands::get_proxy_auth_settings,
            commands::save_proxy_auth_settings,
            commands::verify_file_hash,
            commands::scan_file_antivirus,
            commands::open_file,
            commands::open_folder,
            commands::reorder_task,
            commands::move_task_up,
            commands::move_task_down,
            commands::get_category_routes,
            commands::set_category_route,
            commands::reset_category_routes,
            commands::get_shell_integration_status,
            commands::set_autostart,
            commands::set_uri_scheme,
            commands::set_context_menu,
            commands::inspect_media_stream,
            commands::start_media_stream_download,
            commands::start_split_media_download,
            commands::get_app_settings,
            commands::get_default_intercept_extensions,
            commands::save_app_settings,
            commands::set_app_language,
            commands::pick_download_folder,
            commands::detect_system_locale,
            commands::get_diagnostic_logs,
            commands::clear_diagnostic_logs,
            commands::export_diagnostic_logs,
            commands::export_state_json,
            commands::import_state_json,
            commands::get_clipboard_url,
            commands::toggle_clipboard_monitor,
            commands::get_clipboard_monitor_status,
            commands::set_clipboard_monitor_active,
            commands::get_clipboard_monitor_state,
            commands::start_file_drag,
            commands::get_browser_integration_status,
            commands::repair_browser_integration,
            commands::open_browser_extensions_page,
            commands::open_extension_folder,
            commands::copy_text_to_clipboard,
            commands::dismiss_browser_onboarding,
            commands::omniget_check_tools,
            commands::omniget_ensure_tools,
            commands::omniget_fetch_formats,
            commands::omniget_start_download,
            commands::take_pending_video_grab,
            commands::get_disk_space_stats,
            commands::check_app_update,
            commands::install_app_update,
            commands::launch_app_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running octoheft application");
}

pub fn resolve_runtime_paths() -> (PathBuf, PathBuf, PathBuf, bool) {
    if ShellIntegration::is_portable_mode() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let data_dir = exe_dir.join("data");
                let logs_dir = data_dir.join("logs");
                let downloads_dir = exe_dir.join("Downloads");
                return (data_dir, logs_dir, downloads_dir, true);
            }
        }
    }

    let app_data = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let local_app_data = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| app_data.clone());

    let storage_dir = app_data.join("octoheft");
    let logs_dir = local_app_data.join("OctoHeft").join("logs");
    let downloads_dir = get_default_downloads_dir();

    (storage_dir, logs_dir, downloads_dir, false)
}

fn get_default_downloads_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            let dl = PathBuf::from(userprofile).join("Downloads");
            if dl.exists() {
                return dl;
            }
        }
    }

    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Downloads")
}
