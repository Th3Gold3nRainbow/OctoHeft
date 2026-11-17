pub mod cookies;
pub mod downloader;
pub mod extractor;
pub mod process;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnigetCookieEntry {
    #[serde(default)]
    pub domain: String,
    #[serde(default = "default_cookie_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub expires: i64,
    #[serde(default)]
    pub http_only: bool,
    pub name: String,
    #[serde(default)]
    pub value: String,
}

fn default_cookie_path() -> String {
    "/".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnigetFormatOption {
    pub id: String,
    pub format_spec: String,
    pub label: String,
    pub resolution: String,
    pub height: u32,
    pub fps: Option<u32>,
    pub container: String,
    pub vcodec: String,
    pub acodec: String,
    pub filesize_bytes: Option<u64>,
    pub audio_only: bool,
    pub recommended: bool,
    #[serde(default)]
    pub direct_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnigetMediaInfo {
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub thumbnail: Option<String>,
    pub duration_seconds: Option<f64>,
    pub duration_formatted: Option<String>,
    pub extractor: Option<String>,
    pub is_direct_file: bool,
    pub formats: Vec<OmnigetFormatOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnigetToolsStatus {
    pub ytdlp_available: bool,
    pub ytdlp_path: Option<String>,
    pub ytdlp_version: Option<String>,
    pub ffmpeg_available: bool,
    pub ffmpeg_path: Option<String>,
    pub ffprobe_path: Option<String>,
    pub js_runtime: Option<String>,
    pub managed_bin_dir: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnigetFetchRequest {
    pub url: String,
    #[serde(default)]
    pub candidate_src: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub cookie_entries: Option<Vec<OmnigetCookieEntry>>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnigetDownloadRequest {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    pub format_spec: String,
    #[serde(default)]
    pub format_id: Option<String>,
    pub container: String,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub vcodec: Option<String>,
    #[serde(default)]
    pub acodec: Option<String>,
    #[serde(default)]
    pub audio_only: bool,
    #[serde(default)]
    pub estimated_bytes: Option<u64>,
    #[serde(default)]
    pub save_dir: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub cookie_entries: Option<Vec<OmnigetCookieEntry>>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub direct_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoGrabberPayload {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub candidate_src: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub cookie_entries: Option<Vec<OmnigetCookieEntry>>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
}

#[derive(Default)]
pub struct PendingVideoGrab {
    inner: Mutex<Option<VideoGrabberPayload>>,
}

impl PendingVideoGrab {
    pub fn set(&self, payload: VideoGrabberPayload) {
        *self.inner.lock() = Some(payload);
    }

    pub fn take(&self) -> Option<VideoGrabberPayload> {
        self.inner.lock().take()
    }
}

