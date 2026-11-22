use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Queued,
    Connecting,
    Downloading,
    Paused,
    Completed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadCategory {
    Compressed,
    Programs,
    Video,
    Audio,
    Documents,
    Other,
}

impl DownloadCategory {
    pub fn from_filename(filename: &str) -> Self {
        let ext = filename
            .rsplit('.')
            .next()
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        match ext.as_str() {
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "img" | "z" => {
                DownloadCategory::Compressed
            }
            "exe" | "msi" | "apk" | "bin" | "dmg" | "deb" | "rpm" | "appimage" => {
                DownloadCategory::Programs
            }
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" | "3gp" => {
                DownloadCategory::Video
            }
            "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" | "wma" => DownloadCategory::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "epub" => {
                DownloadCategory::Documents
            }
            _ => DownloadCategory::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegmentStatus {
    Pending,
    Downloading,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentProgress {
    pub id: usize,
    pub start_byte: u64,
    pub end_byte: u64,
    pub current_byte: u64,
    pub downloaded: u64,
    pub speed: u64,
    pub status: SegmentStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaStreamInfo {
    pub protocol: String, // "HLS" | "DASH" | "YT-DLP"
    pub video_resolution: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<u32>,
    pub total_segments: usize,
    pub completed_segments: usize,
    pub mux_phase: String,
    pub mux_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_container: Option<String>,
    #[serde(default)]
    pub audio_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: String,
    pub url: String,
    pub file_name: String,
    pub save_path: String,
    pub total_size: u64,
    pub downloaded_bytes: u64,
    pub speed: u64,
    pub eta_seconds: Option<u64>,
    pub status: DownloadStatus,
    pub category: DownloadCategory,
    pub supports_range: bool,
    pub connections: usize,
    pub segments: Vec<SegmentProgress>,
    pub error_message: Option<String>,
    #[serde(default)]
    pub order_index: i64,
    #[serde(default)]
    pub media_info: Option<MediaStreamInfo>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgressPayload {
    pub id: String,
    pub status: DownloadStatus,
    pub downloaded_bytes: u64,
    pub total_size: u64,
    pub speed: u64,
    pub eta_seconds: Option<u64>,
    pub segments: Vec<SegmentProgress>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub media_info: Option<MediaStreamInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDownloadRequest {
    pub url: String,
    pub save_path: Option<String>,
    pub file_name: Option<String>,
    pub connections: Option<usize>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default, alias = "organize_by_category", alias = "create_category_subfolders")]
    pub use_category_subfolders: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_category_detection() {
        assert_eq!(
            DownloadCategory::from_filename("archive.tar.gz"),
            DownloadCategory::Compressed
        );
        assert_eq!(
            DownloadCategory::from_filename("setup.exe"),
            DownloadCategory::Programs
        );
        assert_eq!(
            DownloadCategory::from_filename("video.mp4"),
            DownloadCategory::Video
        );
        assert_eq!(
            DownloadCategory::from_filename("song.flac"),
            DownloadCategory::Audio
        );
        assert_eq!(
            DownloadCategory::from_filename("document.pdf"),
            DownloadCategory::Documents
        );
        assert_eq!(
            DownloadCategory::from_filename("data.unknown"),
            DownloadCategory::Other
        );
    }
}
