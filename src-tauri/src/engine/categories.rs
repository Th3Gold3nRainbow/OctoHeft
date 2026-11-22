use crate::engine::types::DownloadCategory;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryFolderConfig {
    pub category: DownloadCategory,
    pub name: String,
    pub relative_path: String,
    pub custom_path: Option<String>,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryRoutes {
    pub base_download_dir: String,
    pub routes: Vec<CategoryFolderConfig>,
}

pub struct CategoryDispatcher {
    base_download_dir: RwLock<PathBuf>,
    use_subfolders: RwLock<bool>,
    storage_path: PathBuf,
    routes: RwLock<HashMap<DownloadCategory, CategoryFolderConfig>>,
}

impl CategoryDispatcher {
    pub fn new(storage_dir: &Path, base_download_dir: PathBuf) -> Self {
        let storage_path = storage_dir.join("categories_state.json");
        let dispatcher = Self {
            base_download_dir: RwLock::new(base_download_dir),
            use_subfolders: RwLock::new(true),
            storage_path,
            routes: RwLock::new(HashMap::new()),
        };

        dispatcher.load_or_init();
        dispatcher
    }

    pub fn apply_folder_settings(&self, new_base_dir: PathBuf, use_subfolders: bool) {
        let old_base = {
            let mut base_guard = self.base_download_dir.write();
            let old = base_guard.clone();
            if !new_base_dir.as_os_str().is_empty() {
                *base_guard = new_base_dir.clone();
            }
            old
        };
        *self.use_subfolders.write() = use_subfolders;

        let active_base = self.base_download_dir.read().clone();
        let mut map = self.routes.write();
        for cfg in map.values_mut() {
            if let Some(ref custom) = cfg.custom_path {
                let p = PathBuf::from(custom);
                let old_default = if cfg.relative_path.is_empty() {
                    old_base.clone()
                } else {
                    old_base.join(&cfg.relative_path)
                };
                if p == old_default || p == old_base {
                    cfg.custom_path = None;
                }
            }
        }
        drop(map);
        let _ = active_base;
    }

    fn default_routes(_base_dir: &Path) -> Vec<CategoryFolderConfig> {
        vec![
            CategoryFolderConfig {
                category: DownloadCategory::Compressed,
                name: "Compressed Archives".to_string(),
                relative_path: "Compressed".to_string(),
                custom_path: None,
                extensions: vec![
                    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "img", "z", "tgz", "cab",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
            CategoryFolderConfig {
                category: DownloadCategory::Programs,
                name: "Programs & Installers".to_string(),
                relative_path: "Programs".to_string(),
                custom_path: None,
                extensions: vec![
                    "exe", "msi", "apk", "bin", "dmg", "deb", "rpm", "appimage", "bat", "cmd", "ps1",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
            CategoryFolderConfig {
                category: DownloadCategory::Video,
                name: "Video & Media".to_string(),
                relative_path: "Video".to_string(),
                custom_path: None,
                extensions: vec![
                    "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "3gp", "ts", "m2ts",
                    "m3u8", "mpd",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
            CategoryFolderConfig {
                category: DownloadCategory::Audio,
                name: "Audio & Music".to_string(),
                relative_path: "Audio".to_string(),
                custom_path: None,
                extensions: vec![
                    "mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus", "aiff",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
            CategoryFolderConfig {
                category: DownloadCategory::Documents,
                name: "Documents & Books".to_string(),
                relative_path: "Documents".to_string(),
                custom_path: None,
                extensions: vec![
                    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "csv", "rtf",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
            CategoryFolderConfig {
                category: DownloadCategory::Other,
                name: "General Downloads".to_string(),
                relative_path: "".to_string(),
                custom_path: None,
                extensions: Vec::new(),
            },
        ]
    }

    fn load_or_init(&self) {
        let base_dir = self.base_download_dir.read().clone();
        if self.storage_path.exists() {
            if let Ok(data) = fs::read_to_string(&self.storage_path) {
                if let Ok(saved) = serde_json::from_str::<Vec<CategoryFolderConfig>>(&data) {
                    let mut map = self.routes.write();
                    for mut cfg in saved {
                        // Migrate legacy default paths where custom_path merely mirrored base_dir/<rel>
                        if let Some(ref custom) = cfg.custom_path {
                            let default_p = if cfg.relative_path.is_empty() {
                                base_dir.clone()
                            } else {
                                base_dir.join(&cfg.relative_path)
                            };
                            if PathBuf::from(custom) == default_p {
                                cfg.custom_path = None;
                            }
                        }
                        map.insert(cfg.category.clone(), cfg);
                    }
                    return;
                }
            }
        }

        // Initialize defaults
        let defaults = Self::default_routes(&base_dir);
        let mut map = self.routes.write();
        for cfg in defaults {
            map.insert(cfg.category.clone(), cfg);
        }
    }

    pub fn save_to_disk(&self) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let list: Vec<CategoryFolderConfig> = self.routes.read().values().cloned().collect();
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = fs::write(&self.storage_path, json);
        }
    }

    /// Classify a filename and/or MIME type into a Category
    pub fn classify(&self, filename: &str, mime_type: Option<&str>) -> DownloadCategory {
        let ext = filename
            .rsplit('.')
            .next()
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        let map = self.routes.read();
        for (cat, cfg) in map.iter() {
            if cfg.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                return cat.clone();
            }
        }

        if let Some(mime) = mime_type {
            let m = mime.to_ascii_lowercase();
            if m.starts_with("video/") {
                return DownloadCategory::Video;
            } else if m.starts_with("audio/") {
                return DownloadCategory::Audio;
            } else if m.starts_with("image/") {
                return DownloadCategory::Documents;
            } else if m.contains("zip")
                || m.contains("tar")
                || m.contains("compressed")
                || m.contains("archive")
            {
                return DownloadCategory::Compressed;
            } else if m.contains("octet-stream") && (ext == "exe" || ext == "msi" || ext == "apk") {
                return DownloadCategory::Programs;
            }
        }

        DownloadCategory::from_filename(filename)
    }

    /// Resolve destination directory for a given category, ensuring directory exists
    pub fn resolve_destination_dir(&self, category: &DownloadCategory) -> PathBuf {
        self.resolve_destination_dir_with_override(category, None)
    }

    /// Resolve destination directory with optional per-download category subfolder override
    pub fn resolve_destination_dir_with_override(
        &self,
        category: &DownloadCategory,
        subfolder_override: Option<bool>,
    ) -> PathBuf {
        let base_dir = self.base_download_dir.read().clone();
        let use_subfolders = subfolder_override.unwrap_or_else(|| *self.use_subfolders.read());

        let target_path = if !use_subfolders {
            base_dir
        } else {
            let map = self.routes.read();
            if let Some(cfg) = map.get(category) {
                if let Some(ref custom) = cfg.custom_path {
                    if !custom.trim().is_empty() {
                        PathBuf::from(custom)
                    } else if !cfg.relative_path.is_empty() {
                        base_dir.join(&cfg.relative_path)
                    } else {
                        base_dir.clone()
                    }
                } else if !cfg.relative_path.is_empty() {
                    base_dir.join(&cfg.relative_path)
                } else {
                    base_dir.clone()
                }
            } else {
                base_dir.clone()
            }
        };

        let _ = fs::create_dir_all(&target_path);
        target_path
    }

    pub fn get_routes(&self) -> CategoryRoutes {
        let base_dir = self.base_download_dir.read().clone();
        let mut list: Vec<CategoryFolderConfig> = self.routes.read().values().cloned().collect();
        // Sort stably: Compressed, Programs, Video, Audio, Documents, Other
        list.sort_by_key(|c| match c.category {
            DownloadCategory::Compressed => 0,
            DownloadCategory::Programs => 1,
            DownloadCategory::Video => 2,
            DownloadCategory::Audio => 3,
            DownloadCategory::Documents => 4,
            DownloadCategory::Other => 5,
        });

        CategoryRoutes {
            base_download_dir: base_dir.to_string_lossy().to_string(),
            routes: list,
        }
    }

    pub fn update_route(
        &self,
        category: DownloadCategory,
        custom_path: Option<String>,
    ) -> Result<(), String> {
        {
            let mut map = self.routes.write();
            if let Some(cfg) = map.get_mut(&category) {
                if let Some(ref path_str) = custom_path {
                    let p = PathBuf::from(path_str);
                    if let Err(e) = fs::create_dir_all(&p) {
                        return Err(format!("Cannot create directory {:?}: {}", p, e));
                    }
                }
                cfg.custom_path = custom_path;
            } else {
                return Err("Category route not found".to_string());
            }
        }
        self.save_to_disk();
        Ok(())
    }

    pub fn reset_routes(&self) {
        let base_dir = self.base_download_dir.read().clone();
        let defaults = Self::default_routes(&base_dir);
        {
            let mut map = self.routes.write();
            map.clear();
            for cfg in defaults {
                map.insert(cfg.category.clone(), cfg);
            }
        }
        self.save_to_disk();
    }
}

