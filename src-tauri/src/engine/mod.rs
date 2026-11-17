pub mod error;
pub mod logging;
pub mod types;
pub mod sparse_file;
pub mod chunk_worker;
pub mod rate_limiter;
pub mod scheduler;
pub mod batch;
pub mod zip_preview;
pub mod proxy_auth;
pub mod post_process;
pub mod state_store;
pub mod categories;
pub mod media_muxer;
pub mod native_host;
pub mod omniget;
pub mod settings;
pub mod clipboard_monitor;
pub mod drag_drop;
pub mod telemetry;
pub mod download_engine;

pub use clipboard_monitor::{ClipboardMonitor, ClipboardUrlDetectedPayload};

pub use error::EngineError;
pub use logging::LogEntry;
pub use types::*;
pub use telemetry::{
    EwmaSpeedTracker, DEFAULT_EWMA_ALPHA, TELEMETRY_TICK_INTERVAL, ZERO_SPEED_GRACE_TICKS,
};
pub use download_engine::DownloadEngine;
pub use state_store::StateStore;
pub use scheduler::{QueueConfig, QueueScheduler, PostDownloadAction};
pub use rate_limiter::TokenBucketLimiter;
pub use batch::expand_pattern;
pub use zip_preview::{ZipFileEntry, ZipPreviewResult, RemoteZipPreviewer};
pub use proxy_auth::{ProxyAuthManager, ProxyAuthSettings, ProxyConfig, ProxyMode, DomainAuth};
pub use post_process::{PostProcessor, HashVerificationResult, AntivirusScanResult};
pub use categories::{CategoryDispatcher, CategoryFolderConfig, CategoryRoutes};
pub use media_muxer::{MediaMuxer, MediaManifestInfo, MediaTrackInfo, MuxProgress};
pub use settings::{detect_system_locale, normalize_language_code, AppSettings, SettingsStore};

