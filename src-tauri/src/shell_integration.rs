use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};
use winreg::enums::*;
use winreg::RegKey;

pub const NATIVE_HOST_ID: &str = "com.antigravity.octoheft";
pub const CHROME_EXTENSION_ID: &str = "kalfiofhhhcdnnkjehidagebfikodflp";
pub const FIREFOX_EXTENSION_ID: &str = "octoheft@antigravity.dev";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserStatusInfo {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub exe_path: Option<String>,
    pub registry_registered: bool,
    pub manifest_exists: bool,
    pub registry_key: String,
    pub extensions_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserIntegrationReport {
    pub browsers: Vec<BrowserStatusInfo>,
    pub extension_dir: String,
    pub extension_dir_exists: bool,
    pub manifest_path: String,
    pub firefox_manifest_path: String,
    pub host_exe_path: String,
    pub onboarding_completed: bool,
    pub all_installed_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OnboardingConfig {
    onboarding_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellIntegrationStatus {
    pub autostart: bool,
    pub uri_scheme: bool,
    pub context_menu: bool,
    pub exe_path: String,
    pub portable_mode: bool,
}

pub struct ShellIntegration;

impl ShellIntegration {
    fn current_exe_string() -> String {
        env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("OctoHeft.exe"))
            .to_string_lossy()
            .to_string()
    }

    pub fn is_portable_mode() -> bool {
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                return exe_dir.join("portable.flag").exists() || exe_dir.join("data").exists();
            }
        }
        false
    }

    /// Check current status of Windows shell integrations
    pub fn get_status() -> ShellIntegrationStatus {
        let exe_path = Self::current_exe_string();
        let autostart = Self::is_autostart_enabled();
        let uri_scheme = Self::is_uri_scheme_enabled();
        let context_menu = Self::is_context_menu_enabled();
        let portable_mode = Self::is_portable_mode();

        ShellIntegrationStatus {
            autostart,
            uri_scheme,
            context_menu,
            exe_path,
            portable_mode,
        }
    }

    // 1. Auto-Start
    pub fn normalize_exe_path_for_compare(raw: &str) -> String {
        let trimmed = raw.trim().trim_matches('"');
        if let Some(clean) = trimmed.strip_prefix(r"\\?\") {
            clean.to_ascii_lowercase()
        } else {
            trimmed.to_ascii_lowercase()
        }
    }

    pub fn extract_exe_from_run_command(cmd: &str) -> String {
        let trimmed = cmd.trim();
        if let Some(rest) = trimmed.strip_prefix('"') {
            if let Some(end_quote) = rest.find('"') {
                return rest[..end_quote].trim().to_string();
            }
            return rest.trim().to_string();
        }

        if let Some(exe_idx) = trimmed.to_ascii_lowercase().find(".exe") {
            let cut = exe_idx + ".exe".len();
            return trimmed[..cut].trim().to_string();
        }

        trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    }

    pub fn format_autostart_command(exe_path: &str) -> String {
        format!("\"{}\" --autostart", exe_path.trim().trim_matches('"'))
    }

    pub fn registry_command_matches_exe(cmd: &str, current_exe: &Path) -> bool {
        let reg_exe = Self::extract_exe_from_run_command(cmd);
        if reg_exe.is_empty() {
            return false;
        }
        let reg_norm = Self::normalize_exe_path_for_compare(&reg_exe);
        let cur_norm = Self::normalize_exe_path_for_compare(&current_exe.to_string_lossy());
        !reg_norm.is_empty() && reg_norm == cur_norm
    }

    pub fn is_autostart_enabled() -> bool {
        let Ok(current_exe) = env::current_exe() else {
            return false;
        };
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run_key) =
            hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        {
            if let Ok(val) = run_key.get_value::<String, _>("OctoHeft") {
                return Self::registry_command_matches_exe(&val, &current_exe);
            }
        }
        false
    }

    pub fn set_autostart(enable: bool) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .map_err(|e| format!("Failed to open Run registry key: {}", e))?;

        if enable {
            let exe = Self::current_exe_string();
            let cmd = Self::format_autostart_command(&exe);
            run_key
                .set_value("OctoHeft", &cmd)
                .map_err(|e| format!("Failed to set Run value: {}", e))?;
        } else {
            match run_key.delete_value("OctoHeft") {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Failed to delete Run value: {}", e)),
            }
        }
        Ok(())
    }

    // 2. URI Protocol Scheme (octoheft://download?url=...)
    pub fn is_uri_scheme_enabled() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey("Software\\Classes\\octoheft\\shell\\open\\command")
            .is_ok()
    }

    pub fn set_uri_scheme(enable: bool) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if enable {
            let (proto_key, _) = hkcu
                .create_subkey("Software\\Classes\\octoheft")
                .map_err(|e| format!("Failed to create octoheft protocol key: {}", e))?;

            proto_key
                .set_value("", &"URL:OctoHeft Protocol")
                .map_err(|e| format!("Failed to set protocol title: {}", e))?;
            proto_key
                .set_value("URL Protocol", &"")
                .map_err(|e| format!("Failed to set URL Protocol value: {}", e))?;

            let (cmd_key, _) = proto_key
                .create_subkey("shell\\open\\command")
                .map_err(|e| format!("Failed to create open command key: {}", e))?;

            let exe = Self::current_exe_string();
            let cmd = format!("\"{}\" \"%1\"", exe);
            cmd_key
                .set_value("", &cmd)
                .map_err(|e| format!("Failed to set open command value: {}", e))?;
        } else {
            let _ = hkcu.delete_subkey_all("Software\\Classes\\octoheft");
        }
        Ok(())
    }

    // 3. Windows Explorer Context Menu ("Verify checksum with OctoHeft")
    pub fn is_context_menu_enabled() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey("Software\\Classes\\*\\shell\\OctoHeftChecksum\\command")
            .is_ok()
    }

    pub fn set_context_menu(enable: bool) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if enable {
            let (menu_key, _) = hkcu
                .create_subkey("Software\\Classes\\*\\shell\\OctoHeftChecksum")
                .map_err(|e| format!("Failed to create shell context menu key: {}", e))?;

            menu_key
                .set_value("", &"Verify checksum with OctoHeft")
                .map_err(|e| format!("Failed to set context menu text: {}", e))?;

            let exe = Self::current_exe_string();
            menu_key
                .set_value("Icon", &exe)
                .map_err(|e| format!("Failed to set context menu icon: {}", e))?;

            let (cmd_key, _) = menu_key
                .create_subkey("command")
                .map_err(|e| format!("Failed to create command key: {}", e))?;

            let cmd = format!("\"{}\" --verify \"%1\"", exe);
            cmd_key
                .set_value("", &cmd)
                .map_err(|e| format!("Failed to set context command value: {}", e))?;
        } else {
            let _ = hkcu.delete_subkey_all("Software\\Classes\\*\\shell\\OctoHeftChecksum");
        }
        Ok(())
    }

    // 4. Safely read clipboard text (with contention retries & Unicode/ANSI fallback)
    pub fn try_read_clipboard_text() -> Result<Option<String>, ()> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::System::DataExchange::{
                CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            };
            use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

            const CF_TEXT: u32 = 1;
            const CF_UNICODETEXT: u32 = 13;
            const MAX_CLIPBOARD_BYTES: usize = 65536;

            unsafe {
                let mut opened = false;
                for attempt in 0..5 {
                    if OpenClipboard(std::ptr::null_mut()) != 0 {
                        opened = true;
                        break;
                    }
                    if attempt < 4 {
                        std::thread::sleep(std::time::Duration::from_millis(12));
                    }
                }
                if !opened {
                    return Err(());
                }

                let mut extracted: Option<String> = None;

                if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
                    let handle = GetClipboardData(CF_UNICODETEXT);
                    if !handle.is_null() {
                        let max_bytes = GlobalSize(handle);
                        let max_u16 = (max_bytes / 2).min(MAX_CLIPBOARD_BYTES / 2);
                        let ptr = GlobalLock(handle) as *const u16;
                        if !ptr.is_null() && max_u16 > 0 {
                            let mut len = 0usize;
                            while len < max_u16 && *ptr.add(len) != 0 {
                                len += 1;
                            }
                            let slice = std::slice::from_raw_parts(ptr, len);
                            extracted = Some(String::from_utf16_lossy(slice));
                            let _ = GlobalUnlock(handle);
                        }
                    }
                }

                if extracted.is_none() && IsClipboardFormatAvailable(CF_TEXT) != 0 {
                    let handle = GetClipboardData(CF_TEXT);
                    if !handle.is_null() {
                        let max_bytes = GlobalSize(handle).min(MAX_CLIPBOARD_BYTES);
                        let ptr = GlobalLock(handle) as *const u8;
                        if !ptr.is_null() && max_bytes > 0 {
                            let mut len = 0usize;
                            while len < max_bytes && *ptr.add(len) != 0 {
                                len += 1;
                            }
                            let slice = std::slice::from_raw_parts(ptr, len);
                            extracted = Some(String::from_utf8_lossy(slice).into_owned());
                            let _ = GlobalUnlock(handle);
                        }
                    }
                }

                let _ = CloseClipboard();
                Ok(extracted)
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(None)
        }
    }

    // Read HTTP/HTTPS URL cleanly from the Windows Clipboard
    pub fn read_clipboard_url() -> Option<String> {
        let raw = Self::try_read_clipboard_text().ok().flatten()?;
        let trimmed = raw.trim().trim_start_matches('\u{feff}').trim();
        let lower_prefix = trimmed.get(..8).unwrap_or(trimmed).to_ascii_lowercase();
        if !trimmed.contains('\n')
            && !trimmed.contains('\r')
            && (lower_prefix.starts_with("http://") || lower_prefix.starts_with("https://"))
        {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    // 5. System-Wide Global Hotkey Listener (Ctrl + Alt + D -> Quick Paste Download)
    pub fn start_global_hotkey_listener(app_handle: tauri::AppHandle) {
        #[cfg(target_os = "windows")]
        {
            std::thread::Builder::new()
                .name("octoheft-hotkey".to_string())
                .spawn(move || {
                    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
                    };
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetMessageW, MSG, WM_HOTKEY,
                    };

                    const HOTKEY_ID: i32 = 0x414C; // 'AL'
                    const VK_D: u32 = 0x44;

                    unsafe {
                        let ok = RegisterHotKey(
                            std::ptr::null_mut(),
                            HOTKEY_ID,
                            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                            VK_D,
                        );

                        if ok == 0 {
                            tracing::warn!(
                                "Could not register global hotkey Ctrl+Alt+D (may be claimed by another process)"
                            );
                            return;
                        }

                        tracing::info!("Registered global hotkey Ctrl+Alt+D for Quick Paste");

                        let mut msg: MSG = std::mem::zeroed();
                        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                            if msg.message == WM_HOTKEY && msg.wParam as i32 == HOTKEY_ID {
                                let url = Self::read_clipboard_url().unwrap_or_default();
                                tracing::info!(
                                    url_present = !url.is_empty(),
                                    "Global hotkey Ctrl+Alt+D triggered"
                                );

                                if let Some(w) = app_handle.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.unminimize();
                                    let _ = w.set_focus();
                                }
                                let _ = app_handle.emit("global-quick-paste", url);
                            }
                        }

                        let _ = UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
                    }
                })
                .ok();
        }
    }

    // 6. Browser Integration Detection, Manifest Generation & Registry Repair
    pub fn resolve_extension_dir() -> PathBuf {
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                for candidate in [
                    exe_dir.join("resources").join("extension"),
                    exe_dir.join("extension"),
                ] {
                    if candidate.join("manifest.json").exists() {
                        return candidate;
                    }
                }
            }
        }

        if let Ok(cwd) = env::current_dir() {
            for candidate in [
                cwd.join("native_messaging").join("extension"),
                cwd.join("..").join("native_messaging").join("extension"),
            ] {
                if candidate.join("manifest.json").exists() {
                    return candidate.canonicalize().unwrap_or(candidate);
                }
            }
        }

        // Fallback expected NSIS target path next to OctoHeft.exe
        env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("resources").join("extension")))
            .unwrap_or_else(|| PathBuf::from("resources").join("extension"))
    }

    fn resolve_manifest_dir(storage_dir: &Path) -> PathBuf {
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let res_dir = exe_dir.join("resources");
                if res_dir.exists() || std::fs::create_dir_all(&res_dir).is_ok() {
                    return res_dir;
                }
            }
        }
        storage_dir.to_path_buf()
    }

    fn onboarding_file(storage_dir: &Path) -> PathBuf {
        storage_dir.join("browser_integration.json")
    }

    pub fn is_onboarding_completed(storage_dir: &Path) -> bool {
        let p = Self::onboarding_file(storage_dir);
        if let Ok(raw) = std::fs::read_to_string(&p) {
            if let Ok(cfg) = serde_json::from_str::<OnboardingConfig>(&raw) {
                return cfg.onboarding_completed;
            }
        }
        false
    }

    pub fn set_onboarding_completed(storage_dir: &Path, completed: bool) -> Result<(), String> {
        let p = Self::onboarding_file(storage_dir);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let cfg = OnboardingConfig {
            onboarding_completed: completed,
        };
        let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        std::fs::write(&p, json).map_err(|e| format!("Failed saving onboarding state: {}", e))
    }

    fn find_browser_exe(exe_name: &str, fallback_paths: &[PathBuf]) -> Option<String> {
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let root = RegKey::predef(hive);
            let subkey = format!(
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}",
                exe_name
            );
            if let Ok(k) = root.open_subkey(&subkey) {
                if let Ok(val) = k.get_value::<String, _>("") {
                    let trimmed = val.trim_matches('"').to_string();
                    if Path::new(&trimmed).exists() {
                        return Some(trimmed);
                    }
                }
            }
        }

        for p in fallback_paths {
            if p.exists() {
                return Some(p.to_string_lossy().to_string());
            }
        }
        None
    }

    fn check_native_host_registry(reg_path: &str, expected_exe: &str) -> (bool, bool) {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(key) = hkcu.open_subkey(reg_path) else {
            return (false, false);
        };
        let Ok(manifest_path_str) = key.get_value::<String, _>("") else {
            return (false, false);
        };
        let manifest_path = Path::new(&manifest_path_str);
        if !manifest_path.exists() {
            return (false, false);
        }

        let valid_target = std::fs::read_to_string(manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(|s| s.to_string()))
            .map(|p| {
                p.eq_ignore_ascii_case(expected_exe)
                    || p.to_ascii_lowercase().ends_with("octoheft.exe")
            })
            .unwrap_or(false);

        (valid_target, true)
    }

    pub fn get_browser_integration_report(storage_dir: &Path) -> BrowserIntegrationReport {
        let host_exe_path = Self::current_exe_string();
        let manifest_dir = Self::resolve_manifest_dir(storage_dir);
        let manifest_path = manifest_dir.join(format!("{}.json", NATIVE_HOST_ID));
        let firefox_manifest_path = manifest_dir.join(format!("{}.firefox.json", NATIVE_HOST_ID));
        let extension_dir = Self::resolve_extension_dir();
        let extension_dir_exists = extension_dir.join("manifest.json").exists();

        let pf = env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string());
        let pf86 = env::var("ProgramFiles(x86)")
            .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
        let local_app = env::var("LOCALAPPDATA").unwrap_or_default();

        let specs = vec![
            (
                "chrome",
                "Google Chrome",
                "chrome.exe",
                vec![
                    PathBuf::from(&pf).join(r"Google\Chrome\Application\chrome.exe"),
                    PathBuf::from(&pf86).join(r"Google\Chrome\Application\chrome.exe"),
                    PathBuf::from(&local_app).join(r"Google\Chrome\Application\chrome.exe"),
                ],
                format!("Software\\Google\\Chrome\\NativeMessagingHosts\\{}", NATIVE_HOST_ID),
                "chrome://extensions",
            ),
            (
                "edge",
                "Microsoft Edge",
                "msedge.exe",
                vec![
                    PathBuf::from(&pf86).join(r"Microsoft\Edge\Application\msedge.exe"),
                    PathBuf::from(&pf).join(r"Microsoft\Edge\Application\msedge.exe"),
                ],
                format!("Software\\Microsoft\\Edge\\NativeMessagingHosts\\{}", NATIVE_HOST_ID),
                "edge://extensions",
            ),
            (
                "brave",
                "Brave Browser",
                "brave.exe",
                vec![
                    PathBuf::from(&pf).join(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
                    PathBuf::from(&pf86).join(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
                    PathBuf::from(&local_app)
                        .join(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
                ],
                format!(
                    "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\{}",
                    NATIVE_HOST_ID
                ),
                "brave://extensions",
            ),
            (
                "firefox",
                "Mozilla Firefox",
                "firefox.exe",
                vec![
                    PathBuf::from(&pf).join(r"Mozilla Firefox\firefox.exe"),
                    PathBuf::from(&pf86).join(r"Mozilla Firefox\firefox.exe"),
                ],
                format!("Software\\Mozilla\\NativeMessagingHosts\\{}", NATIVE_HOST_ID),
                "about:debugging#/runtime/this-firefox",
            ),
        ];

        let mut browsers = Vec::with_capacity(specs.len());
        let mut all_installed_connected = true;
        let mut any_installed = false;

        for (id, name, exe_name, fallbacks, reg_subkey, ext_url) in specs {
            let exe_path = Self::find_browser_exe(exe_name, &fallbacks);
            let installed = exe_path.is_some();
            let (registry_registered, manifest_exists) =
                Self::check_native_host_registry(&reg_subkey, &host_exe_path);

            if installed {
                any_installed = true;
                if !registry_registered || !manifest_exists {
                    all_installed_connected = false;
                }
            }

            browsers.push(BrowserStatusInfo {
                id: id.to_string(),
                name: name.to_string(),
                installed,
                exe_path,
                registry_registered,
                manifest_exists,
                registry_key: format!("HKCU\\{}", reg_subkey),
                extensions_url: ext_url.to_string(),
            });
        }

        if !any_installed {
            all_installed_connected = browsers.iter().all(|b| b.registry_registered);
        }

        BrowserIntegrationReport {
            browsers,
            extension_dir: extension_dir.to_string_lossy().to_string(),
            extension_dir_exists,
            manifest_path: manifest_path.to_string_lossy().to_string(),
            firefox_manifest_path: firefox_manifest_path.to_string_lossy().to_string(),
            host_exe_path,
            onboarding_completed: Self::is_onboarding_completed(storage_dir),
            all_installed_connected,
        }
    }

    pub fn repair_browser_integration(
        storage_dir: &Path,
    ) -> Result<BrowserIntegrationReport, String> {
        let host_exe_path = Self::current_exe_string();
        let manifest_dir = Self::resolve_manifest_dir(storage_dir);
        std::fs::create_dir_all(&manifest_dir)
            .map_err(|e| format!("Failed creating manifest directory: {}", e))?;

        let chromium_manifest_path = manifest_dir.join(format!("{}.json", NATIVE_HOST_ID));
        let firefox_manifest_path = manifest_dir.join(format!("{}.firefox.json", NATIVE_HOST_ID));

        let chromium_json = serde_json::json!({
            "name": NATIVE_HOST_ID,
            "description": "OctoHeft Modern Download Accelerator Native Messaging Host",
            "path": host_exe_path,
            "type": "stdio",
            "allowed_origins": [
                format!("chrome-extension://{}/", CHROME_EXTENSION_ID)
            ]
        });

        let firefox_json = serde_json::json!({
            "name": NATIVE_HOST_ID,
            "description": "OctoHeft Modern Download Accelerator Native Messaging Host",
            "path": host_exe_path,
            "type": "stdio",
            "allowed_extensions": [
                FIREFOX_EXTENSION_ID
            ]
        });

        std::fs::write(
            &chromium_manifest_path,
            serde_json::to_string_pretty(&chromium_json).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("Failed writing Chromium Native Host JSON: {}", e))?;

        std::fs::write(
            &firefox_manifest_path,
            serde_json::to_string_pretty(&firefox_json).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("Failed writing Firefox Native Host JSON: {}", e))?;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let chromium_str = chromium_manifest_path.to_string_lossy().to_string();
        let firefox_str = firefox_manifest_path.to_string_lossy().to_string();

        for subkey in [
            format!("Software\\Google\\Chrome\\NativeMessagingHosts\\{}", NATIVE_HOST_ID),
            format!("Software\\Microsoft\\Edge\\NativeMessagingHosts\\{}", NATIVE_HOST_ID),
            format!(
                "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\{}",
                NATIVE_HOST_ID
            ),
        ] {
            let (k, _) = hkcu
                .create_subkey(&subkey)
                .map_err(|e| format!("Registry write failed ({}): {}", subkey, e))?;
            k.set_value("", &chromium_str)
                .map_err(|e| format!("Registry value failed ({}): {}", subkey, e))?;
        }

        let ff_subkey = format!("Software\\Mozilla\\NativeMessagingHosts\\{}", NATIVE_HOST_ID);
        let (fk, _) = hkcu
            .create_subkey(&ff_subkey)
            .map_err(|e| format!("Registry write failed ({}): {}", ff_subkey, e))?;
        fk.set_value("", &firefox_str)
            .map_err(|e| format!("Registry value failed ({}): {}", ff_subkey, e))?;

        tracing::info!(
            chromium_manifest = %chromium_str,
            firefox_manifest = %firefox_str,
            "Repaired Windows NativeMessagingHosts registry keys"
        );

        Ok(Self::get_browser_integration_report(storage_dir))
    }

    pub fn open_browser_extensions_page(
        storage_dir: &Path,
        browser_id: Option<&str>,
    ) -> Result<(), String> {
        let report = Self::get_browser_integration_report(storage_dir);

        let target = if let Some(id) = browser_id {
            report
                .browsers
                .iter()
                .find(|b| b.id == id && b.installed)
                .cloned()
        } else {
            report
                .browsers
                .iter()
                .find(|b| b.installed && b.id != "firefox")
                .or_else(|| report.browsers.iter().find(|b| b.installed))
                .cloned()
        };

        if let Some(browser) = target {
            if let Some(exe) = browser.exe_path {
                std::process::Command::new(exe)
                    .arg(&browser.extensions_url)
                    .spawn()
                    .map_err(|e| format!("Failed to launch {}: {}", browser.name, e))?;
                return Ok(());
            }
        }

        Err("No compatible browser (Chrome, Edge, Brave, Firefox) was detected on this system.".to_string())
    }

    pub fn write_clipboard_text(text: &str) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::System::DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
            };
            use windows_sys::Win32::System::Memory::{
                GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
            };

            const CF_UNICODETEXT: u32 = 13;
            let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let byte_len = utf16.len() * 2;

            unsafe {
                if OpenClipboard(std::ptr::null_mut()) == 0 {
                    return Err("Could not open Windows clipboard".to_string());
                }
                let _ = EmptyClipboard();

                let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
                if hmem.is_null() {
                    let _ = CloseClipboard();
                    return Err("Clipboard memory allocation failed".to_string());
                }

                let ptr = GlobalLock(hmem) as *mut u16;
                if ptr.is_null() {
                    let _ = CloseClipboard();
                    return Err("Clipboard GlobalLock failed".to_string());
                }

                std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
                let _ = GlobalUnlock(hmem);

                if SetClipboardData(CF_UNICODETEXT, hmem as _).is_null() {
                    let _ = CloseClipboard();
                    return Err("SetClipboardData failed".to_string());
                }

                let _ = CloseClipboard();
            }
            Ok(())
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = text;
            Ok(())
        }
    }

    /// Native Windows Shell Folder Picker dialog using Win32 `SHBrowseForFolderW`.
    pub fn pick_folder_dialog(title: &str) -> Result<Option<String>, String> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::System::Com::{
                CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
            };
            use windows_sys::Win32::UI::Shell::{
                SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE,
                BIF_RETURNONLYFSDIRS, BROWSEINFOW,
            };

            let title_utf16: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let mut display_name = [0u16; 260];
            let mut path_buf = [0u16; 520];

            unsafe {
                let hr = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
                let mut bi: BROWSEINFOW = std::mem::zeroed();
                bi.pszDisplayName = display_name.as_mut_ptr();
                bi.lpszTitle = title_utf16.as_ptr();
                bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;

                let pidl = SHBrowseForFolderW(&mut bi);
                let mut result: Option<String> = None;
                if !pidl.is_null() {
                    if SHGetPathFromIDListW(pidl, path_buf.as_mut_ptr()) != 0 {
                        let len = path_buf.iter().position(|&c| c == 0).unwrap_or(0);
                        let selected = String::from_utf16_lossy(&path_buf[..len]);
                        if !selected.trim().is_empty() {
                            result = Some(selected);
                        }
                    }
                    CoTaskMemFree(pidl as *const std::ffi::c_void);
                }

                if hr >= 0 {
                    CoUninitialize();
                }

                Ok(result)
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = title;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShellIntegration;
    use std::path::Path;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    #[test]
    fn test_autostart_command_formatting_and_exe_matching() {
        let exe = Path::new(r"C:\Program Files\OctoHeft\OctoHeft.exe");
        let cmd = ShellIntegration::format_autostart_command(&exe.to_string_lossy());
        assert_eq!(cmd, r#""C:\Program Files\OctoHeft\OctoHeft.exe" --autostart"#);

        assert!(ShellIntegration::registry_command_matches_exe(&cmd, exe));
        assert!(ShellIntegration::registry_command_matches_exe(
            r#""c:\program files\octoheft\octoheft.exe" --minimized"#,
            exe
        ));
        assert!(ShellIntegration::registry_command_matches_exe(
            r"C:\Program Files\OctoHeft\OctoHeft.exe --autostart",
            exe
        ));

        // Stale location in another folder must NOT count as matching active exe
        assert!(!ShellIntegration::registry_command_matches_exe(
            r#""C:\OldFolder\OctoHeft.exe" --autostart"#,
            exe
        ));
        assert!(!ShellIntegration::registry_command_matches_exe("", exe));
    }

    #[test]
    fn test_hkcu_temp_registry_roundtrip_without_touching_run_key() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let subkey = format!("Software\\OctoHeft\\TestRunProbe_{}", uuid::Uuid::new_v4());
        let (key, _) = hkcu.create_subkey(&subkey).expect("create probe subkey");
        let expected_exe = Path::new(r"C:\Users\Test\AppData\Local\OctoHeft\OctoHeft.exe");
        let expected_cmd =
            ShellIntegration::format_autostart_command(&expected_exe.to_string_lossy());
        key.set_value("OctoHeft", &expected_cmd)
            .expect("write probe value");

        let read_back: String = key.get_value("OctoHeft").expect("read probe value");
        assert!(ShellIntegration::registry_command_matches_exe(
            &read_back,
            expected_exe
        ));

        let _ = key.delete_value("OctoHeft");
        let _ = hkcu.delete_subkey_all(&subkey);
    }
}

