export type DownloadStatus =
  | 'queued'
  | 'connecting'
  | 'downloading'
  | 'paused'
  | 'completed'
  | 'error';

export type DownloadCategory =
  | 'all'
  | 'compressed'
  | 'programs'
  | 'video'
  | 'audio'
  | 'documents'
  | 'other';

export type SegmentStatus = 'pending' | 'downloading' | 'completed' | 'failed';

export interface SegmentProgress {
  id: number;
  start_byte: number;
  end_byte: number;
  current_byte: number;
  downloaded: number;
  speed: number;
  status: SegmentStatus;
}

export interface MediaStreamInfo {
  protocol: string; // "HLS" | "DASH" | "YT-MSE" | "YT-DLP"
  video_resolution?: string;
  video_codec?: string;
  audio_codec?: string;
  audio_bitrate?: number; // kbps
  total_segments: number;
  completed_segments: number;
  mux_phase: string; // "parsing" | "downloading" | "demuxing" | "muxing" | "completed" | "error"
  mux_percent: number;
  format_spec?: string;
  source_url?: string;
  target_container?: string;
  audio_only?: boolean;
}

export interface OmnigetCookieEntry {
  domain: string;
  path: string;
  secure: boolean;
  expires: number;
  http_only: boolean;
  name: string;
  value: string;
}

export interface OmnigetFormatOption {
  id: string;
  format_spec: string;
  label: string;
  resolution: string;
  height: number;
  fps?: number | null;
  container: string;
  vcodec: string;
  acodec: string;
  filesize_bytes?: number | null;
  audio_only: boolean;
  recommended: boolean;
  direct_url?: string | null;
}

export interface OmnigetMediaInfo {
  url: string;
  title: string;
  uploader?: string | null;
  thumbnail?: string | null;
  duration_seconds?: number | null;
  duration_formatted?: string | null;
  extractor?: string | null;
  is_direct_file: boolean;
  formats: OmnigetFormatOption[];
}

export interface OmnigetToolsStatus {
  ytdlp_available: boolean;
  ytdlp_path?: string | null;
  ytdlp_version?: string | null;
  ffmpeg_available: boolean;
  ffmpeg_path?: string | null;
  ffprobe_path?: string | null;
  js_runtime?: string | null;
  managed_bin_dir: string;
  ready: boolean;
}

export interface VideoGrabberPayload {
  url: string;
  title?: string | null;
  candidate_src?: string | null;
  cookies?: string | null;
  cookie_entries?: OmnigetCookieEntry[] | null;
  user_agent?: string | null;
  referrer?: string | null;
}

export interface DownloadTask {
  id: string;
  url: string;
  file_name: string;
  save_path: string;
  total_size: number;
  downloaded_bytes: number;
  speed: number;
  eta_seconds: number | null;
  status: DownloadStatus;
  category: DownloadCategory;
  supports_range: boolean;
  connections: number;
  segments: SegmentProgress[];
  error_message?: string;
  order_index?: number;
  media_info?: MediaStreamInfo;
  created_at: string;
  updated_at: string;
}

export interface DownloadProgressPayload {
  id: string;
  status: DownloadStatus;
  downloaded_bytes: number;
  total_size: number;
  speed: number;
  eta_seconds: number | null;
  segments: SegmentProgress[];
  error_message?: string;
  media_info?: MediaStreamInfo;
}

export interface AddDownloadRequest {
  url: string;
  save_path?: string;
  file_name?: string;
  connections?: number;
}

export type PostDownloadAction = 'none' | 'exitapp' | 'sleeppc' | 'shutdownpc';

export interface QueueConfig {
  id: string;
  name: string;
  max_concurrent_downloads: number;
  start_time: string | null;
  stop_time: string | null;
  days_of_week: number;
  post_action: PostDownloadAction;
  is_enabled: boolean;
  task_ids: string[];
}

export interface ZipFileEntry {
  name: string;
  uncompressed_size: number;
  compressed_size: number;
  compression_method: number;
  compression_name: string;
  crc32: number;
  local_header_offset: number;
  is_directory: boolean;
}

export interface ZipPreviewResult {
  url: string;
  total_archive_size: number;
  entries_count: number;
  files: ZipFileEntry[];
}

export type ProxyMode = 'system' | 'none' | 'manual';

export interface ProxyConfig {
  mode: ProxyMode;
  server_url?: string;
  username?: string;
  password?: string;
}

export interface DomainAuth {
  domain: string;
  username?: string;
  password?: string;
  bearer_token?: string;
  custom_headers: Record<string, string>;
}

export interface ProxyAuthSettings {
  proxy: ProxyConfig;
  domain_auths: DomainAuth[];
  custom_user_agent?: string;
}

export interface HashVerificationResult {
  computed_sha256: string;
  computed_md5: string;
  expected_hash?: string;
  is_match?: boolean;
}

export interface AntivirusScanResult {
  scanner_name: string;
  is_clean: boolean;
  exit_code?: number;
  details: string;
}

export interface CategoryFolderConfig {
  category: DownloadCategory;
  name: string;
  relative_path: string;
  custom_path?: string;
  extensions: string[];
}

export interface CategoryRoutes {
  base_download_dir: string;
  routes: CategoryFolderConfig[];
}

export interface ShellIntegrationStatus {
  autostart: boolean;
  uri_scheme: boolean;
  context_menu: boolean;
  exe_path: string;
  portable_mode: boolean;
}

export interface LogEntry {
  timestamp: string;
  level: string;
  target: string;
  message: string;
}

export interface ImportStateResult {
  tasks_imported: number;
  queues_imported: number;
}

export interface MediaTrackInfo {
  track_type: string;
  codec: string;
  resolution?: string;
  bandwidth: number;
  init_url?: string;
  segment_urls: string[];
}

export interface MediaManifestInfo {
  protocol: string;
  video_tracks: MediaTrackInfo[];
  audio_tracks: MediaTrackInfo[];
  total_duration_secs: number;
  recommended_video_resolution?: string;
  recommended_audio_bitrate?: number;
}

export interface BrowserStatusInfo {
  id: string;
  name: string;
  installed: boolean;
  exe_path?: string;
  registry_registered: boolean;
  manifest_exists: boolean;
  registry_key: string;
  extensions_url: string;
}

export interface BrowserIntegrationReport {
  browsers: BrowserStatusInfo[];
  extension_dir: string;
  extension_dir_exists: boolean;
  manifest_path: string;
  firefox_manifest_path: string;
  host_exe_path: string;
  onboarding_completed: boolean;
  all_installed_connected: boolean;
}

export interface AppSettings {
  language: string;
  intercept_extensions: string;
  sound_on_complete: boolean;
  default_download_dir: string;
  use_category_subfolders: boolean;
  clipboard_monitor_enabled?: boolean;
  max_connections_per_download?: number;
}

export interface ClipboardUrlDetectedPayload {
  url: string;
  filename?: string | null;
  is_media: boolean;
}

export interface DiskSpaceStats {
  drive_label: string;
  target_path: string;
  available_bytes: number;
  total_bytes: number;
  used_bytes: number;
  used_percent: number;
}


