use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const SUPPORTED_LANGUAGES: [&str; 7] = ["en", "fr", "es", "de", "ar", "ru", "zh"];

pub const LEGACY_DEFAULT_INTERCEPT_EXTENSIONS: &str = "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV Z ZIP PKG";

pub const DEFAULT_INTERCEPT_EXTENSIONS: &str = "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV Z ZIP PKG DAT";

fn default_true() -> bool {
    true
}

fn default_intercept_extensions() -> String {
    DEFAULT_INTERCEPT_EXTENSIONS.to_string()
}

pub fn default_download_dir_string() -> String {
    #[cfg(target_os = "windows")]
    {
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            let dl = PathBuf::from(userprofile).join("Downloads");
            return dl.to_string_lossy().to_string();
        }
    }

    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Downloads")
        .to_string_lossy()
        .to_string()
}

fn default_chunks() -> u32 {
    8
}

fn detect_initial_language() -> String {
    if let Some(reg_lang) = read_registry_language() {
        return reg_lang;
    }
    detect_system_locale()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "detect_initial_language")]
    pub language: String,
    #[serde(default = "default_intercept_extensions")]
    pub intercept_extensions: String,
    #[serde(default = "default_true")]
    pub sound_on_complete: bool,
    #[serde(default = "default_download_dir_string")]
    pub default_download_dir: String,
    #[serde(default = "default_true")]
    pub use_category_subfolders: bool,
    #[serde(default)]
    pub clipboard_monitor_enabled: bool,
    #[serde(default = "default_chunks", alias = "parallel_connections", alias = "chunks_per_download")]
    pub max_connections_per_download: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: detect_initial_language(),
            intercept_extensions: default_intercept_extensions(),
            sound_on_complete: true,
            default_download_dir: default_download_dir_string(),
            use_category_subfolders: true,
            clipboard_monitor_enabled: false,
            max_connections_per_download: 8,
        }
    }
}

pub fn normalize_language_code(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let primary = lower
        .split(&['-', '_'][..])
        .next()
        .unwrap_or("en")
        .trim();

    match primary {
        "fr" => "fr".to_string(),
        "es" => "es".to_string(),
        "de" => "de".to_string(),
        "ar" => "ar".to_string(),
        "ru" => "ru".to_string(),
        "zh" => "zh".to_string(),
        _ => "en".to_string(),
    }
}

pub fn normalize_intercept_extensions(raw: &str) -> String {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    for part in raw.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let mut clean = part.trim().trim_start_matches('*').trim_start_matches('.').to_ascii_uppercase();
        clean.retain(|c| c.is_ascii_alphanumeric() || c == '*');
        if clean.is_empty() {
            continue;
        }
        if seen.insert(clean.clone()) {
            tokens.push(clean);
        }
    }
    tokens.join(" ")
}

/// Read HKCU\Software\OctoHeft\Language if written by the NSIS installer or settings UI.
pub fn read_registry_language() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey("Software\\OctoHeft") {
            if let Ok(val) = key.get_value::<String, _>("Language") {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return Some(normalize_language_code(trimmed));
                }
            }
        }
    }
    None
}

/// Persist the active language into HKCU\Software\OctoHeft\Language so installer & app agree.
pub fn write_registry_language(language: &str) {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        let normalized = normalize_language_code(language);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok((key, _)) = hkcu.create_subkey("Software\\OctoHeft") {
            let _ = key.set_value("Language", &normalized);
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = language;
}

/// Consume a one-shot InstallerLanguageOverride value written by the NSIS setup wizard so
/// reinstalling with an explicit installer language (e.g. English on a French OS) overrides
/// any stale settings_state.json once without overriding later user choices in Settings UI.
pub fn take_installer_language_override() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey_with_flags("Software\\OctoHeft", KEY_READ | KEY_WRITE) {
            if let Ok(val) = key.get_value::<String, _>("InstallerLanguageOverride") {
                let _ = key.delete_value("InstallerLanguageOverride");
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return Some(normalize_language_code(trimmed));
                }
            }
        }
    }
    None
}

/// Detect the active Windows user UI locale via `GetUserDefaultLocaleName` (`Win32_Globalization`).
pub fn detect_system_locale() -> String {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

        let mut buf = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if len > 1 {
            let slice = &buf[..(len as usize - 1)];
            let locale = String::from_utf16_lossy(slice);
            return normalize_language_code(&locale);
        }
    }

    if let Ok(lang_env) = std::env::var("LANG") {
        if !lang_env.trim().is_empty() {
            return normalize_language_code(&lang_env);
        }
    }

    "en".to_string()
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(storage_dir: &Path) -> Self {
        Self {
            path: storage_dir.join("settings_state.json"),
        }
    }

    fn sanitize(settings: &AppSettings) -> AppSettings {
        let dir = settings.default_download_dir.trim();
        let default_dir = if dir.is_empty() {
            default_download_dir_string()
        } else {
            dir.to_string()
        };

        let mut normalized_exts = normalize_intercept_extensions(&settings.intercept_extensions);
        if normalized_exts == LEGACY_DEFAULT_INTERCEPT_EXTENSIONS {
            normalized_exts = DEFAULT_INTERCEPT_EXTENSIONS.to_string();
        }

        let connections = if settings.max_connections_per_download == 0 {
            8
        } else {
            settings.max_connections_per_download.clamp(1, 16)
        };

        AppSettings {
            language: normalize_language_code(&settings.language),
            intercept_extensions: normalized_exts,
            sound_on_complete: settings.sound_on_complete,
            default_download_dir: default_dir,
            use_category_subfolders: settings.use_category_subfolders,
            clipboard_monitor_enabled: settings.clipboard_monitor_enabled,
            max_connections_per_download: connections,
        }
    }

    pub fn load_or_init_with_registry_override(
        &self,
        registry_language_override: Option<String>,
    ) -> AppSettings {
        self.load_or_init_with_overrides(registry_language_override, None)
    }

    pub fn load_or_init_with_overrides(
        &self,
        registry_language_override: Option<String>,
        one_shot_installer_override: Option<String>,
    ) -> AppSettings {
        if self.path.exists() {
            if let Ok(raw) = std::fs::read_to_string(&self.path) {
                if let Ok(mut parsed) = serde_json::from_str::<AppSettings>(&raw) {
                    if let Some(installer_lang) = one_shot_installer_override
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        parsed.language = normalize_language_code(installer_lang);
                        let clean = Self::sanitize(&parsed);
                        let _ = self.save(&clean);
                        return clean;
                    }
                    parsed.language = normalize_language_code(&parsed.language);
                    return Self::sanitize(&parsed);
                }
            }
        }

        let initial_lang = one_shot_installer_override
            .or(registry_language_override)
            .filter(|s| !s.trim().is_empty())
            .map(|s| normalize_language_code(&s))
            .unwrap_or_else(detect_system_locale);

        let detected = AppSettings {
            language: initial_lang,
            intercept_extensions: default_intercept_extensions(),
            sound_on_complete: true,
            default_download_dir: default_download_dir_string(),
            use_category_subfolders: true,
            clipboard_monitor_enabled: false,
            max_connections_per_download: 8,
        };
        let _ = self.save(&detected);
        detected
    }

    pub fn load_or_init(&self) -> AppSettings {
        let one_shot = take_installer_language_override();
        self.load_or_init_with_overrides(read_registry_language(), one_shot)
    }

    pub fn save(&self, settings: &AppSettings) -> Result<AppSettings, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create settings directory: {}", e))?;
        }

        let clean = Self::sanitize(settings);
        let json = serde_json::to_string_pretty(&clean)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        std::fs::write(&self.path, json)
            .map_err(|e| format!("Failed to write settings_state.json: {}", e))?;
        Ok(clean)
    }

    pub fn set_language(&self, language: &str) -> Result<AppSettings, String> {
        let mut current = self.load_or_init();
        current.language = normalize_language_code(language);
        self.save(&current)
    }

    pub fn set_clipboard_monitor_enabled(&self, enabled: bool) -> Result<AppSettings, String> {
        let mut current = self.load_or_init();
        current.clipboard_monitor_enabled = enabled;
        self.save(&current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_locale_normalization_for_all_supported_languages() {
        assert_eq!(normalize_language_code("en-US"), "en");
        assert_eq!(normalize_language_code("fr-FR"), "fr");
        assert_eq!(normalize_language_code("es-MX"), "es");
        assert_eq!(normalize_language_code("de-DE"), "de");
        assert_eq!(normalize_language_code("ar-SA"), "ar");
        assert_eq!(normalize_language_code("ru-RU"), "ru");
        assert_eq!(normalize_language_code("zh-CN"), "zh");
        assert_eq!(normalize_language_code("zh_TW"), "zh");
        assert_eq!(normalize_language_code("pt-BR"), "en");
    }

    #[test]
    fn test_settings_persistence_and_partial_nsis_json_migration() {
        let dir = std::env::temp_dir().join(format!("octoheft_settings_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Simulate partial JSON written by NSIS installer when user selected English (1033)
        std::fs::write(dir.join("settings_state.json"), r#"{"language":"en"}"#).unwrap();

        let store = SettingsStore::new(&dir);
        let initial = store.load_or_init_with_registry_override(Some("fr".to_string()));
        assert_eq!(initial.language, "en");
        assert_eq!(initial.intercept_extensions, DEFAULT_INTERCEPT_EXTENSIONS);
        assert!(initial.sound_on_complete);
        assert!(initial.use_category_subfolders);
        assert!(!initial.default_download_dir.is_empty());
        assert_eq!(initial.max_connections_per_download, 8);

        let saved = store
            .save(&AppSettings {
                language: "ar-EG".to_string(),
                intercept_extensions: ".mp4, *.mkv r0* r1* PKG".to_string(),
                sound_on_complete: false,
                default_download_dir: dir.to_string_lossy().to_string(),
                use_category_subfolders: false,
                clipboard_monitor_enabled: true,
                max_connections_per_download: 12,
            })
            .expect("save failed");
        assert_eq!(saved.language, "ar");
        assert_eq!(saved.intercept_extensions, "MP4 MKV R0* R1* PKG");
        assert!(!saved.sound_on_complete);
        assert!(!saved.use_category_subfolders);
        assert!(saved.clipboard_monitor_enabled);
        assert_eq!(saved.max_connections_per_download, 12);

        let reloaded = store.load_or_init_with_registry_override(None);
        assert_eq!(reloaded.language, "ar");
        assert_eq!(reloaded.intercept_extensions, "MP4 MKV R0* R1* PKG");
        assert!(!reloaded.sound_on_complete);
        assert!(!reloaded.use_category_subfolders);
        assert!(reloaded.clipboard_monitor_enabled);
        assert_eq!(reloaded.max_connections_per_download, 12);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_registry_language_overrides_os_locale_when_settings_absent() {
        let dir = std::env::temp_dir().join(format!("octoheft_reglang_{}", Uuid::new_v4()));
        let store = SettingsStore::new(&dir);

        // First boot with HKCU\Software\OctoHeft\Language = "de" overrides OS locale
        let loaded = store.load_or_init_with_registry_override(Some("de".to_string()));
        assert_eq!(loaded.language, "de");

        // Reinstalling with one-shot installer override updates stale settings_state.json once
        let overridden = store.load_or_init_with_overrides(None, Some("en".to_string()));
        assert_eq!(overridden.language, "en");
        // Subsequent launch without one-shot override preserves persisted setting
        let persisted = store.load_or_init_with_overrides(Some("fr".to_string()), None);
        assert_eq!(persisted.language, "en");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_default_intercept_extensions_contains_expected_tokens() {
        let tokens: Vec<&str> = DEFAULT_INTERCEPT_EXTENSIONS.split_whitespace().collect();
        assert_eq!(tokens.len(), 54);
        assert!(tokens.contains(&"3GP"));
        assert!(tokens.contains(&"R0*"));
        assert!(tokens.contains(&"R1*"));
        assert!(tokens.contains(&"PKG"));
        assert!(tokens.contains(&"ZIP"));
        assert!(tokens.contains(&"DAT"));
    }
}
