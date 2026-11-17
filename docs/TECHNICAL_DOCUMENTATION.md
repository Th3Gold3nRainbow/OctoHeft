# OctoHeft Technical Architecture & Engineering Reference

**Product:** OctoHeft (Crate: `octoheft-engine` v0.1.0, UI Shell v2.0)  
**Bundle Identifier:** `com.antigravity.octoheft`  
**Target Platform:** Windows 10 & Windows 11 (x64, `PerMonitorV2` DPI-aware)  
**Methodology:** Clean-Room Reverse Architecture Specification (REA-Prompts Framework, Section 4)  
**Primary Runtime Stack:** Rust (2021 Edition), Tauri v2, Tokio, Reqwest (`rustls`), Win32/COM (`windows-sys`), React 18, TypeScript, Tailwind CSS v3  

---

## Table of Contents

1. [Executive Summary & Genesis (OctoHeft vs. Internet Download Manager)](#1-executive-summary--genesis-octoheft-vs-internet-download-manager)
2. [Architecture & Core Systems](#2-architecture--core-systems)
3. [Native Windows 11 Integration](#3-native-windows-11-integration)
4. [Browser Extension & Native Messaging Protocol](#4-browser-extension--native-messaging-protocol)
5. [Technical Specifications & Benchmarks](#5-technical-specifications--benchmarks)
6. [Demonstration Video Script & Walkthrough Guide](#6-demonstration-video-script--walkthrough-guide)

---

## 1. Executive Summary & Genesis (OctoHeft vs. Internet Download Manager)

### 1.1 Architectural Motivation & Clean-Room Methodology

OctoHeft is a clean-room Windows desktop download accelerator and adaptive media acquisition engine designed according to Section 4 (Software & Feature Cloning) of the REA-Prompts reverse-architecture methodology. Rather than wrapping legacy command-line wrappers around an electron shell or patching closed-source helpers, OctoHeft re-engineers the high-value behavioral primitives pioneered by classic Windows download managers—multi-connection byte-range segmentation, live clipboard and browser download interception, outbound file drag-and-drop, queue scheduling, post-download power orchestration, and category routing—onto a modern, memory-safe Rust asynchronous core paired with an Edge WebView2 presentation layer.

Legacy architectures such as Internet Download Manager (IDM) trace their foundations to 32-bit C++/MFC applications built for Windows XP/7 I/O semantics, WinInet/Winsock synchronous socket pools, fixed-DPI bitmap skinning, and standalone native messaging helper executables (`IDMMsgHost.exe`). While proven over decades, those structural choices introduce measurable friction on modern Windows 11 x64 workstations: high-bitrate NVMe storage stalls during post-download segment concatenation, fragile browser-extension maintenance via ad-hoc DOM cipher scraping, blurry or clipped GDI widgets under mixed Per-Monitor DPI scaling, and multi-binary installation state split across Windows Registry hives and helper daemons.

OctoHeft addresses each of these architectural constraints at the systems level while matching or exceeding expected Windows desktop ergonomics.

### 1.2 Architectural Comparison Matrix: OctoHeft vs. Legacy IDM

| Architectural Dimension | Internet Download Manager (Legacy Reference) | OctoHeft Architecture |
| :--- | :--- | :--- |
| **Execution Model & Footprint** | 32-bit Win32/MFC monolith plus separate helper binaries (`IDMan.exe`, `IEMonitor.exe`, `IDMMsgHost.exe`) | Single dual-role x64 native binary (`OctoHeft.exe`, `6,003,200` bytes released) combining GUI daemon and headless Native Messaging host |
| **Async Network Engine** | Fixed native socket worker threads with proprietary HTTP/TLS stack | Tokio multi-threaded async runtime (`reqwest 0.12` + `rustls-tls`), HTTP/2 connection pooling (`32` idle sockets/host), SOCKS5/HTTP proxy support |
| **File Pre-Allocation & Completion** | Downloads segments into separate temporary part files and performs sequential concatenation upon reaching 99% | Pre-allocates full target file via `set_len` + Win32 NTFS `FSCTL_SET_SPARSE` (`0x000900C4`) and executes lock-free positional `FileExt::seek_write`; **0 ms assembly pass** |
| **Media Acquisition & Remuxing** | Built-in proprietary FLV/TS/MP4 muxer frequently broken by YouTube SABR, cipher updates, and rolling signatures | Dual hybrid pipeline: pure-Rust ISOBMFF/EBML/HLS/DASH core (`media_muxer.rs`) plus `omniget/` orchestrator managing SHA-256-verified `yt-dlp`, `FFmpeg`, `FFprobe`, and optional `deno` |
| **UI Rendering & DPI Scaling** | Legacy GDI bitmap toolbars and modal MFC dialogs; prone to clipping on 150%+ fractional DPI monitors | Tauri v2 + Microsoft Edge WebView2 Fluent UI (`Segoe UI Variable`), NSIS `PerMonitorV2` manifest, CSS Container Queries (`@container`), `subgrid`, RTL mirroring |
| **Browser Extension Model** | Manifest V2/V3 extension talking to external `IDMMsgHost.exe` broker | Deterministic MV3 key (`kalfiofhhhcdnnkjehidagebfikodflp`); background worker spawns `OctoHeft.exe` in headless stdio mode bridging directly to `\\.\pipe\OctoHeftNativeIpc` |
| **Outbound OS Drag & Drop** | Legacy Win32 list-view `DoDragDrop` | Dual COM STA pipeline on WebView2: Shell PIDL `SHCreateDataObject` + `SHDoDragDrop`, with pure-Rust vtable `CfHdropDataObject` + `DoDragDrop` fallback |
| **State & Configuration Storage** | Mixed binary structures and nested Windows Registry keys under `HKCU\Software\DownloadManager` | Atomic UTF-8 JSON stores guarded by `parking_lot::RwLock` (`tasks_state.json`, `queues_state.json`, `categories_state.json`, `settings_state.json`, `proxy_auth_settings.json`) |
| **Installer & Portable Operation** | Classic Windows setup modifying system integration keys | LZMA-compressed NSIS setup (`OctoHeft_0.1.0_x64-setup.exe`, `2,624,896` bytes) plus zero-registry Portable Mode via `./portable.flag` or `./data/` directory |

### 1.3 Eliminating the Legacy "99% Concatenation Freeze"

A well-known bottleneck in legacy segmented download managers occurs when a multi-gigabyte file completes its final HTTP byte range. Because segments are buffered into isolated temporary chunk files during transfer, the application must open all temporary parts and stream-copy tens of gigabytes sequentially into a newly created destination file at `99%` progress—temporarily doubling peak disk space consumption and saturating drive I/O queues.

OctoHeft eliminates this phase in `src-tauri/src/engine/sparse_file.rs`:

1. **Zero-Cost Virtual Allocation**: Upon probing `Content-Length`, `SparseFile::create_preallocated` sets the logical file length (`file.set_len(total_size)`) and marks the NTFS handle sparse using `DeviceIoControl(..., FSCTL_SET_SPARSE, ...)`. Sparse NTFS attributes prevent the kernel from synchronously zero-filling unallocated gigabytes on open while reserving deterministic byte offsets.
2. **Shared Lock-Free Positional Writes**: A single opened `std::fs::File` handle wrapped in `Arc<File>` is shared across up to `32` concurrent Tokio segment workers. Each worker writes directly into its assigned `[start_byte, end_byte]` region via Windows overlapped positional semantics (`std::os::windows::fs::FileExt::seek_write`).
3. **Instantaneous Completion**: Once the final chunk returns and flushes, `sparse_file.sync_all()` commits file metadata without moving or copying a single byte. File completion transitions directly from active transfer to `Completed` with zero temporary assembly artifacts.

### 1.4 Monolithic Dual-Role Executable & Distribution Footprint

Rather than shipping separate CLI bridge processes beside the desktop application, `src-tauri/src/main.rs` inspects `std::env::args()` prior to initializing Tauri or Microsoft Edge WebView2:

- **Native Messaging Entrypoint (`--native-host`, `chrome-extension://`, `moz-extension://`, `--parent-window=`, or host manifest filename)**: Invokes `native_host::run_native_host_stdio()` inside a lightweight `tokio::runtime::Builder::new_current_thread()` runtime. It reads and writes 32-bit little-endian length-prefixed JSON frames over `stdin`/`stdout` and communicates with any active GUI instance over Windows Named Pipe `\\.\pipe\OctoHeftNativeIpc`. WebView2 and GDI/GUI subsystems are never initialized in this path.
- **Desktop GUI Entrypoint**: Launches `octoheft_engine_lib::run()`, initializes tracing logs, state stores, download engine, scheduler, system tray, clipboard watcher, global hotkey pump, Named Pipe IPC server, and the Tauri WebView2 window.

Measured release artifacts produced from `Cargo.toml` (`opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`):
- **Standalone Dual-Role Binary (`OctoHeft.exe`)**: `6,003,200` bytes (`~5.73 MiB`)
- **NSIS Solid LZMA Installer (`OctoHeft_0.1.0_x64-setup.exe`)**: `2,624,896` bytes (`~2.50 MiB`)

---

## 2. Architecture & Core Systems

### 2.1 High-Level Component & Concurrency Topology

```text
+-----------------------------------------------------------------------------------+
|                        Microsoft Edge WebView2 / React 18 UI                      |
|  App.tsx | Header.tsx | Sidebar.tsx | DownloadTable.tsx | VideoDownloaderAddon    |
+-----------------------------------------+-----------------------------------------+
                                          | Tauri v2 IPC (66 Commands + Events)
                                          v
+-----------------------------------------------------------------------------------+
|                       OctoHeft Core Runtime (src-tauri/src/lib.rs)                |
|                                                                                   |
|  +-----------------------+  +-----------------------+  +-----------------------+  |
|  | DownloadEngine        |  | QueueScheduler (1s)   |  | ClipboardMonitor      |  |
|  |  - ChunkWorker (<=32) |  |  - Concurrency Caps   |  |  - 500ms Sequence     |  |
|  |  - SparseFile (NTFS)  |  |  - Time/Day Windows   |  |  - Win32 Foreground   |  |
|  |  - TokenBucketLimiter |  |  - Sleep/Shutdown PC  |  |  - 54 Ext + Media URL |  |
|  +-----------+-----------+  +-----------+-----------+  +-----------+-----------+  |
|              |                          |                          |              |
|  +-----------v--------------------------v--------------------------v-----------+  |
|  |                  Shared JSON Persistence & Synchronization                  |  |
|  |  StateStore | SettingsStore | CategoryDispatcher | ProxyAuthManager         |  |
|  +--------------------------------------+--------------------------------------+  |
|                                         |                                         |
|  +-----------------------+  +-----------v-----------+  +-----------------------+  |
|  | RemoteZipPreviewer    |  | OctoMedia Dual Engine |  | NativePipeIPC Server  |  |
|  |  - EOCD Tail Range    |  |  - media_muxer (Rust) |  |  \\.\pipe\            |  |
|  |  - Deflate/Store Out  |  |  - omniget/ (yt-dlp,  |  |  OctoHeftNativeIpc    |  |
|  +-----------------------+  |    FFmpeg, FFprobe)   |  +-----------+-----------+  |
|                             +-----------------------+              ^              |
+--------------------------------------------------------------------|--------------+
                                                                     |
                                         +---------------------------+--------------+
                                         | OctoHeft.exe (Headless Native Host Mode) |
                                         | Stdin/Stdout 32-bit LE Length Frames     |
                                         +---------------------------^--------------+
                                                                     |
                                         +---------------------------+--------------+
                                         | Chrome / Edge / Brave / Firefox MV3 Ext  |
                                         +------------------------------------------+
```

All shared stores use `parking_lot::RwLock` guards and serialize directly into human-readable JSON state files within the resolved storage root (`%APPDATA%\octoheft` in standard installs, or `./data` in Portable Mode).

### 2.2 Segmented Download Engine & Chunk Workers

The HTTP/HTTPS transfer pipeline is implemented across `src-tauri/src/engine/download_engine.rs`, `chunk_worker.rs`, and `rate_limiter.rs`.

#### URL Probing & Dynamic Segmentation
1. **`DownloadEngine::probe_url` (`download_engine.rs`)**:
   - Builds a `reqwest::Client` configured with `rustls-tls`, `connect_timeout(20s)`, `pool_max_idle_per_host(32)`, and a Chrome/138 User-Agent (`DEFAULT_BROWSER_USER_AGENT`). Proxy settings from `ProxyAuthManager` are applied per request configuration.
   - Issues an initial `HEAD` request to obtain `Content-Length`, `Accept-Ranges`, `Content-Type`, and `Content-Disposition` (supporting RFC 5987 `filename*=` and quoted `filename=`).
   - If the origin rejects `HEAD` (`405`/`501`/opaque response) or omits content length, falls back immediately to a 1-byte range probe (`GET` with `Range: bytes=0-0`), extracting total entity length from the `Content-Range: bytes 0-0/<total>` header and confirming server seekability.
2. **Segment Partitioning (`calculate_initial_segments`)**:
   - Multi-connection slicing activates when `supports_range == true` and `total_size >= 524_288` bytes (`512 KiB`).
   - User-requested thread counts (`connections`) are clamped cleanly into `[1, 32]` (defaulting to `8` streams).
   - Non-rangeable streams or resources under `512 KiB` default safely to a single segment (`SegmentProgress { id: 0, ... }`).

```text
start_download_internal()
   |
   +--> probe_url() [HEAD -> fallback GET Range: bytes=0-0]
   +--> CategoryDispatcher::classify() + resolve_destination_dir()
   +--> SparseFile::create_preallocated(path, total_size)
   +--> ActiveTaskControl registration (watch::Sender<bool>, per-task SpeedLimiter)
   |
   +===> Spawn N ChunkWorker tasks (tokio::spawn, N in 1..=32)
   |        |-- Each worker reads ChunkRange [start_byte .. end_byte]
   |        |-- NO_REDIRECT_CLIENT manual redirect loop (<= 6 hops)
   |        |-- Streams body chunks -> SpeedLimiter::acquire(n) -> SparseFile::write_at()
   |        |-- Transient network/5xx/429 retry <= 5x (300 * 2^retry ms)
   |        +-- Fatal HTTP 401/403/404/410 aborts immediately
   |
   +--> Progress Aggregator loop (throttled >= 150 ms -> "download-progress" IPC event)
   +--> sparse_file.sync_all() -> Completed (0 ms file assembly)
```

#### Header Hygiene, Cookie Redirect Gating & Cancellation
In `chunk_worker.rs`, outbound headers pass through `normalize_request_headers`:
- Strips caller-injected hop-by-hop or conflicting transport headers (`range`, `host`, `content-length`, `connection`, `accept-encoding`).
- Canonicalizes header casing (`Cookie`, `Referer`, `User-Agent`, `Origin`, `Authorization`).
- Enforces `Referer: https://www.youtube.com/` and `Origin: https://www.youtube.com` automatically on `.googlevideo.com` CDN hosts to prevent CDN token rejections.
- **`NO_REDIRECT_CLIENT` (`redirect::Policy::none()`)**: Rather than letting `reqwest` auto-follow cross-domain redirects with sensitive headers intact, `ChunkWorker` manually resolves `301`, `302`, `303`, `307`, and `308` hops (up to `6` redirects), stripping `Cookie` headers whenever `should_forward_cookie_on_redirect` determines the target crosses untrusted host boundaries (retaining cookies only on identical/subdomain hosts or trusted `youtube.com`/`googlevideo.com` pairs).
- Cancellation uses a `tokio::sync::watch::channel(false)` flag inside `ActiveTaskControl`; pausing closes socket streams gracefully, preserves exact `current_byte` per segment inside `tasks_state.json`, and allows exact byte-offset resumption across application restarts.

#### Lock-Free Fast-Path Token Bucket Rate Limiter (`rate_limiter.rs`)
`TokenBucketLimiter` enforces both a global bandwidth ceiling and per-task ceilings simultaneously:
- **Zero-Overhead Bypass**: Reads `rate_bytes_per_sec: AtomicU64` (`Ordering::Relaxed`). When set to `0` (Unlimited), `acquire()` returns immediately without acquiring any mutex.
- **Burst & Refill**: Under active throttling, refills floating-point token balance bounded by `max_burst = max(rate * 2.0, 131_072.0)` (`128 KiB`). When tokens are insufficient, calculates the deficit wait interval, releases the `tokio::sync::Mutex` immediately, and awaits `tokio::time::sleep(wait_duration)` so sibling workers never suffer mutex starvation.

### 2.3 NTFS Sparse File Subsystem

Defined in `src-tauri/src/engine/sparse_file.rs`, `SparseFile` wraps `Arc<std::fs::File>` alongside an atomic sparse flag:
- Opening in write/create/read mode, it calls `file.set_len(total_size)` when `total_size > 0`.
- On Windows targets (`#[cfg(target_os = "windows")]`), it retrieves the raw OS `HANDLE` (`AsRawHandle`) and issues `DeviceIoControl(handle, 0x000900C4 /* FSCTL_SET_SPARSE */, null_mut(), 0, null_mut(), 0, &mut bytes_returned, null_mut())`. If the target volume is non-NTFS (e.g., exFAT/FAT32 USB media), non-sparse fallback proceeds cleanly without failing the download.
- Worker writes execute via `self.file.seek_write(data, offset)` (`std::os::windows::fs::FileExt`), writing directly at arbitrary file offsets thread-safely without external mutex serialization around file seeks.

### 2.4 Remote ZIP Inspector & Selective Extraction

`src-tauri/src/engine/zip_preview.rs` (`RemoteZipPreviewer`) inspects remote `.zip` archives without downloading the full archive body:

```text
Remote .ZIP Binary Layout                     OctoHeft Range Fetches
+-----------------------------------+
| Local File Header (0x04034b50)    |<--- Step 4: Selective Extract Fetch
| [File N Compressed Payload]       |     (Single Range: local_offset..end)
+-----------------------------------+
| ...                               |
+-----------------------------------+
| Central Directory Records         |<--- Step 3: Slice from tail buffer or
| (0x02014b50 per entry)            |     fetch [cd_offset .. cd_offset+cd_size]
+-----------------------------------+
| End of Central Directory (EOCD)   |<--- Step 1: HEAD Content-Length
| Signature: 0x06054b50             |<--- Step 2: GET Range trailing min(size, 64 KB)
+-----------------------------------+
```

1. **Tail Probe**: Fetches `HEAD Content-Length` and requests `Range: bytes=<total_size - tail_len>-<total_size - 1>`, where `tail_len = min(total_size, 65_536)`.
2. **EOCD & Central Directory Parsing**: Scans backward for the 4-byte signature `0x06054b50`, extracts `cd_size` and `cd_offset`, re-uses the cached tail buffer if the Central Directory already resides inside the 64 KiB suffix (otherwise fetches the exact `cd_offset` range), and iterates `0x02014b50` headers to enumerate all enclosed entries (skipping Zip64/invalid offsets cleanly and returning entry name, compressed/uncompressed sizes, CRC-32, compression method, and `local_header_offset`).
3. **Selective Extraction (`extract_single_entry`)**: Requests `[local_header_offset .. local_header_offset + 30 + fname + extra + compressed_size + 1024]`, validates `0x04034b50`, advances past variable-length filename and extra fields, and either copies raw bytes (`Method 0 = Stored`) or streams through `flate2::read::DeflateDecoder` (`Method 8 = Deflated`) directly onto disk.

### 2.5 Queue Scheduler, Categorizer, Proxy/Auth, Batch Expander & Post-Processing

- **Queue Scheduler (`scheduler.rs`)**: Runs on a 1-second Tokio interval evaluating `QueueConfig` definitions persisted in `queues_state.json`. Each queue specifies `max_concurrent_downloads`, `start_time` (`HH:MM`), `stop_time` (`HH:MM`), `days_of_week` (7-bit mask where bit `0` = Sunday through bit `6` = Saturday), and `post_action` (`DoNothing`, `ExitApp`, `SleepPC` executing `rundll32.exe powrprof.dll,SetSuspendState 0,1,0`, or `ShutdownPC` executing `shutdown.exe /s /t 30`). Supports both native HTTP downloads and `YT-DLP` Omniget tasks (`resume_omniget_task`).
- **Automatic Category Dispatcher (`categories.rs`)**: Classifies transfers into six categories (`Compressed`, `Programs`, `Video`, `Audio`, `Documents`, `Other`) using file extension maps with MIME-type fallback (`application/zip`, `video/*`, `audio/*`, `application/pdf`, etc.). Routes completed destinations dynamically according to `use_category_subfolders` and custom per-category overrides saved in `categories_state.json`.
- **Batch URL Pattern Expander (`batch.rs`)**: Expands numeric intervals (`[1-100]` or zero-padded `[001-050]`, preserving leading-zero width) and ASCII alphabetic sequences (`[a-z]`, `[A-Z]`), enforced at a `1,000`-URL generation cap per batch.
- **Proxy & Domain Authentication (`proxy_auth.rs`)**: Supports `ProxyMode::System`, `ProxyMode::None`, and `ProxyMode::Manual` (HTTP and `socks5://` URIs with credentials), paired with per-domain authentication entries (`Basic`, `Bearer`, custom header injections) stored in `proxy_auth_settings.json`.
- **Cryptographic Integrity & Defender Scanning (`post_process.rs`)**: `PostProcessor::compute_file_hashes` performs a single linear file pass using a `128 KiB` `BufReader` and a `64 KiB` stack buffer (`[0u8; 65_536]`) to feed `sha2::Sha256` and `md5::Md5` simultaneously. `PostProcessor::scan_file` locates Microsoft Defender `MpCmdRun.exe` (under `ProgramData\Microsoft\Windows Defender\Platform\<version>\` or `Program Files\Windows Defender\`) or a user-specified scanner binary and runs `-Scan -ScanType 3 -File "<path>"`.

### 2.6 OctoMedia Dual Pipeline (`media_muxer.rs` & `omniget/`)

Legacy download managers rely heavily on fragile DOM scraping of player JSON (`ytInitialPlayerResponse`) and obfuscated JavaScript signature-deciphering routines that fail with HTTP `403 Forbidden` or output unplayable VP9-in-MP4 streams whenever YouTube rotates player scripts or enforces SABR. OctoHeft removes all fragile DOM/cipher hacks and replaces them with a two-tier media architecture:

1. **Native Pure-Rust Stream Engine (`src-tauri/src/engine/media_muxer.rs`, 6,436 lines)**:
   - Handles direct HLS (`.m3u8` master/media playlists), MPEG-DASH (`.mpd`), and split A/V MSE streams (`dispatch_split_media_task` for direct captured video+audio track pairs).
   - Implements memory-efficient ISOBMFF (`ftyp`, `moov`, `mdat`, `moof`/`traf`/`trun` via `mp4 = "0.14"` + custom box readers), MPEG-TS/ADTS/H.264/HEVC NALU demuxers, and custom EBML/Matroska/WebM muxing—requiring **zero external binaries** for HLS/DASH/MSE stream assembly.
2. **Omniget Managed Extraction & Remuxing Engine (`src-tauri/src/engine/omniget/`)**:
   - **`process.rs`**: `ResolvedTools::discover()` probes Managed Bin Dir (`%LOCALAPPDATA%\OctoHeft\bin` or `./data/bin` in Portable Mode), executable sibling folder, WinGet package links/directories, Scoop shims, `Program Files`, and `PATH` for `yt-dlp.exe`, `ffmpeg.exe`, `ffprobe.exe`, and `deno.exe`. When absent, `ensure_tools_installed()` fetches official upstream releases (`yt-dlp.exe` + `SHA2-256SUMS`, Gyan FFmpeg essentials zip + `.sha256`, and optional `deno` MSVC zip + `.sha256`), validates SHA-256 digests locally via `sha2::Sha256`, strips inherited `PYTHONHOME`/`PYTHONPATH` variables, forces `PYTHONIOENCODING=utf-8` / `PYTHONUTF8=1`, prepends `augmented_path_env()`, hides consoles via `CREATE_NO_WINDOW` (`0x0800_0000`), and tears down active subprocess trees cleanly on cancel via `taskkill /PID <pid> /T /F`.
   - **`cookies.rs`**: `TempCookieFile` serializes structured `OmnigetCookieEntry` records (and raw cookie header fallback pairs) into Netscape cookie-jar format (including `#HttpOnly_` prefixes and control-character stripping), guaranteeing automatic file deletion via RAII `Drop`.
   - **`extractor.rs`**: Executes `yt-dlp --dump-single-json` under a `45`-second timeout using a 3-tier YouTube cascade (`youtube` default w/ cookies -> `youtube:player_client=default,web_safari,mweb` if progressive resolution caps at `<= 360p` -> cookieless fallback when cookies trigger account/age challenges -> `try_direct_http_probe` fallback). Maps codec pairings cleanly into lossless containers:
     - `AVC` + `AAC` -> `.mp4`
     - `VP9` / `VP8` + `Opus` -> `.webm`
     - `AV1` / `HEVC` (or mixed-codec high-res pairs) -> `.mkv`
     - Audio-only presets -> `M4A`, `Opus`, or `MP3`
   - **`downloader.rs`**: Spawns `yt-dlp --continue -N 8 --progress-delta 0.25` with structured templates (`OCTO_PROG|...`, `OCTO_POST|...`, `OCTO_FILEPATH:...`), throttles UI IPC events at `160 ms`, responds immediately to pause/cancel via `tokio::select!`, and verifies every output file post-mux using `ffprobe -v error -show_entries stream=codec_type,codec_name,width,height -of json` (`VerifiedMediaProbe`).

```text
Browser / UI ("Fetch Formats")
       |
       v
[omniget/extractor.rs]
  |-- Strategy 1: yt-dlp --dump-single-json + TempCookieFile (45s timeout)
  |-- Strategy 2 (if YT & max_height <= 360p): player_client=default,web_safari,mweb
  |-- Strategy 3 (if cookies fail/degrade): cookieless cascade
  +-- Fallback (if yt-dlp absent/non-page): direct HTTP HEAD/Range media probe
       |
       v
Curated Quality Matrix (4K/1440p/1080p .mkv/.webm/.mp4 + Audio M4A/Opus/MP3)
       |  User picks quality -> "Download Selected Quality"
       v
[omniget/downloader.rs]
  |-- yt-dlp --continue -N 8 -> OCTO_PROG| / OCTO_POST| -> 160ms UI IPC updates
  |-- FFmpeg lossless remux (zero re-encoding when codecs match container)
  +-- ffprobe stream verification -> Completed DownloadTask in StateStore
```

---

## 3. Native Windows 11 Integration

### 3.1 Outbound Win32 OLE/Shell Drag-and-Drop (`drag_drop.rs` & `DownloadTable.tsx`)

WebView2's HTML5 `DataTransfer` sandbox cannot emit native Explorer `CF_HDROP` drop payloads across process boundaries onto the Windows Desktop or File Explorer. OctoHeft bridges this cleanly using native OLE COM drag-and-drop:

1. **Path Normalization & UI-Thread STA Marshaling (`drag_drop.rs`)**:
   - `normalize_win32_path` strips Windows extended-length prefixes (`\\?\` and `\\?\UNC\`) because Shell PIDL and `DROPFILES` handlers expect standard Win32 or UNC paths.
   - `start_file_drag` validates on-disk file existence (`validate_drag_file_path`) and dispatches execution onto the main Win32 Single-Threaded Apartment (STA) UI thread via `main_window.run_on_main_thread(...)`, awaiting completion over a `tokio::sync::oneshot` channel.
2. **Primary Shell PIDL Drag (`SHCreateDataObject` + `SHDoDragDrop`)**:
   - Calls `OleInitialize(null())`.
   - Builds a full PIDL via `ILCreateFromPathW`, clones it (`ILClone`), strips the leaf item (`ILRemoveLastID`) to obtain the parent folder PIDL alongside the child item (`ILFindLastID`), constructs a full Explorer Shell data object via `SHCreateDataObject`, and invokes `SHDoDragDrop(hwnd, shell_data_obj, drop_src, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect)`.
3. **Pure-Rust COM `CF_HDROP` Fallback (`CfHdropDataObject` + `DoDragDrop`)**:
   - Should Shell PIDL resolution fail (e.g., virtual/special paths), `execute_drag_on_ui_thread` falls back to a hand-built `#[repr(C)]` COM `IDataObject` implementation (`CfHdropDataObject` with 12-slot `IDataObjectVtbl`) and `SimpleDropSource` (`IDropSourceVtbl`, responding `DRAGDROP_S_CANCEL` on `Escape`, `DRAGDROP_S_DROP` on left-button release, `DRAGDROP_S_USEDEFAULTCURSORS` on feedback).
   - Encodes a binary 20-byte `DROPFILES { pFiles: 20, fWide: 1, ... }` header followed by UTF-16LE paths terminated by a double NUL (`0x0000, 0x0000`), copied into `GMEM_MOVEABLE` `HGLOBAL` memory and enumerated via `SHCreateStdEnumFmtEtc`.
4. **Coexistence with In-Table Priority Reordering (`DownloadTable.tsx`)**:
   - Completed rows (`data-outbound-drag="true"`) and their category icon wrappers intercept `onDragStart` with `e.preventDefault(); e.stopPropagation();`, set `html.is-file-dragging` (`cursor: grabbing !important;`), and call `invoke('start_file_drag', { filePath })`.
   - The inner `<GripVertical />` drag handle in Column 1 calls `e.stopPropagation()` before calling `e.dataTransfer.setData('text/plain', task.id)`, ensuring row priority reordering remains isolated from OS-level file dragging.

### 3.2 Race-Free Clipboard Monitor & Foreground Focus Restoration

Implemented in `src-tauri/src/engine/clipboard_monitor.rs` and `shell_integration.rs`:

- **Synchronized Toggle & State Persistence**: Toggling the Clipboard Monitor button in `Header.tsx` updates both `ClipboardMonitor` (`MONITOR_ACTIVE: AtomicBool`) and `SettingsStore::clipboard_monitor_enabled`, broadcasting `settings-updated` across all UI subscribers.
- **Non-Destructive Polling (`500 ms` Tokio Worker)**: Uses Win32 `GetClipboardSequenceNumber()` as a zero-allocation change check. When the sequence changes, `spawn_blocking(ShellIntegration::try_read_clipboard_text)` attempts `OpenClipboard` (up to `5` retries spaced `12 ms` apart to survive Transient lock contention by browsers/editors, capped at `65,536` bytes, checking `CF_UNICODETEXT` before `CF_TEXT`). Crucially, `last_seq` is **only** updated after `OpenClipboard` succeeds—guaranteeing temporary clipboard locks retry automatically on the next 500 ms tick rather than dropping copied URLs.
- **Deduplication & Re-Copy Semantics**: `set_active(true)` snapshots the current OS sequence number (preventing stale pre-existing clipboard contents from popping immediately) while clearing `last_copied_text: Mutex<String>`. Copying an identical URL consecutively is ignored, whereas copying any non-matching text/URL replaces `last_copied_text` so re-copying the download URL afterward triggers reliably.
- **URL Classification & Disposition Parsing**:
  - `url::Url::path()` isolates path segments cleanly before `?query` and `#fragment` markers (`test_archive.rar?auth=token#sec` -> `.rar`).
  - Also parses query parameters (`filename`, `file`, `name`, `download`, `response-content-disposition`) for S3/R2/CDN signed links.
  - Matches all `54` default extensions (case-insensitive, plus multipart wildcards `R0*`/`R1*` -> `.r00`..`.r99`) as well as direct manifests (`.m3u8`, `.mpd`) and media hosts (`youtube.com`, `youtu.be`, `vimeo.com`, `twitch.tv`, `dailymotion.com`, `dai.ly`, `tiktok.com`, `instagram.com`, `x.com`, `twitter.com`, `facebook.com`, `fb.watch`, `bilibili.com`, `b23.tv`, `rumble.com`, `soundcloud.com`).
- **Foreground Activation (`bring_window_to_foreground_win32`)**: To overcome Windows Desktop Window Manager (DWM) background focus-stealing restrictions, `restore_and_focus_main_window` executes on the main UI thread: restores iconic windows (`SW_RESTORE`/`SW_SHOW`), temporarily joins the current foreground thread queue via `AttachThreadInput`, performs an `HWND_TOPMOST` -> `HWND_NOTOPMOST` z-order pulse with `SetWindowPos`, calls `BringWindowToTop` and `SetForegroundWindow`, detaches `AttachThreadInput`, and emits `clipboard-url-detected` to prefill `AddDownloadModal` (or switch directly into `OctoMedia` if `is_media == true`).

### 3.3 System Tray Lifecycle, Close-to-Tray, Global Hotkey & Portable Mode

- **Race-Free Tray Restoration (`tray.rs`)**: Windows tray handlers historically suffer hide/show flicker when binding both `MouseDown` and `MouseUp` or blindly inverting visibility (`if visible { hide } else { show }`). OctoHeft disables left-click menu popup (`.show_menu_on_left_click(false)`) and runs a strict idempotent `restore_main_window` (`unminimize()` -> `show()` -> `set_focus()`) exclusively on `TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up }` and `TrayIconEvent::DoubleClick { button: MouseButton::Left }`. Right-click opens a 7-language context menu with `Show`, `Hide`, `Start with Windows` (`CheckMenuItem` backed by `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\OctoHeft` -> `"<exe>" --autostart`), and `Quit`.
- **Close-to-Tray & Silent Boot (`lib.rs`)**: Intercepts `WindowEvent::CloseRequested` to `window.hide()` + `api.prevent_close()`, keeping background queues, Named Pipe IPC, and clipboard monitoring alive. Launching with `--minimized` or `--autostart` hides the main window at setup without flashing the UI.
- **Global Quick-Paste Hotkey (`shell_integration.rs`)**: Spawns dedicated OS thread `octoheft-hotkey` registering `Ctrl+Alt+D` (`MOD_CONTROL | MOD_ALT`, `VK_D = 0x44`, `HOTKEY_ID = 0x414C`) with a native `GetMessageW` pump, restoring the window and emitting `global-quick-paste` (populated from clipboard if valid HTTP/HTTPS).
- **Zero-Footprint Portable Mode (`lib.rs`, `shell_integration.rs`)**: `is_portable_mode()` checks whether `portable.flag` or a sibling `data/` folder exists next to `OctoHeft.exe`. When active, `resolve_runtime_paths()` redirects state storage to `./data/`, logs to `./data/logs/`, managed OctoMedia binaries to `./data/bin/`, and default downloads to `./Downloads/` adjacent to the executable.

### 3.4 Responsive Fluent UI, Container Queries & 3-Way Theme Engine

Frontend styles (`src/index.css`, `tailwind.config.js`) and layout components (`Header.tsx`, `Sidebar.tsx`, `DownloadTable.tsx`, `VideoDownloaderAddon.tsx`, `useTheme.ts`) leverage modern browser CSS primitives supported natively inside Windows 11 Edge WebView2:

- **3-Column `subgrid` Header (`Header.tsx`, `index.css`)**: Uses `.octoheft-header` (`container-type: inline-size; container-name: header`) and `.octoheft-header-toolbar` (`grid-template-columns: subgrid`). At container widths `< 920px`, the center action group wraps cleanly onto row 2; at `>= 920px`, search bar, center action bar (`Live Speed Pill`, `+ Add URL`, `Resume All`, `Pause All`), vertical separator, and frosted Utility Pill (`Speed Limiter` portal popover, `Batch`, `Scheduler`, `ZIP Preview`, `Clipboard Monitor`, `3-Way Theme Toggle`, `Settings`) align across one responsive row. Includes roving `focusgroup` keyboard navigation (`ArrowRight`/`ArrowLeft`/`Home`/`End`, RTL-aware).
- **Fixed Table & Scroll-Driven Truncation Reveal (`DownloadTable.tsx`, `index.css`)**: `.downloads-table` enforces `table-layout: fixed` with proportional `<colgroup>` widths to eliminate horizontal clipping of the `Actions` column. At `@container downloads-table (inline-size < 780px)`, rows transform cleanly into accessible stacked cards with sticky file headers. Long filenames use CSS custom property `--truncated` (`@property`), `animation-timeline: scroll(self inline)`, and `ResizeObserver` fallback to trigger a smooth 2-second single-line horizontal pan on `:hover` or `:focus-visible` (RTL-aware).
- **3-Way Theme Cycling (`useTheme.ts`)**: Cycles `'light' -> 'dark' -> 'neon'`, persisted via `localStorage.octoheft_theme`, synchronized via `StorageEvent` and `CustomEvent('octoheft-theme-change')`, toggling `html.dark` and `html.neon` classes (mapped via custom Tailwind variant `addVariant('neon', 'html.neon &')` for Deep Cyber Navy `#00122e` surfaces and `#00f0ff` cyan accents).
- **Live Disk Space Telemetry (`commands.rs`, `Sidebar.tsx`)**: `query_disk_space_stats` calls Win32 `GetDiskFreeSpaceExW` (walking up nonexistent parent directories until an existing mount point is found) on startup, settings update, download completion, and every `15` seconds during active transfers, driving a native styled `<progress class="octoheft-disk-progress">` bar that flips animation direction under `:dir(rtl)` and shifts fill color at `>= 80%` (`#d97706` amber) and `>= 92%` (`#e11d48` rose).

---

## 4. Browser Extension & Native Messaging Protocol

### 4.1 Manifest V3 Extension Architecture (`native_messaging/extension/`)

- **Deterministic Cryptographic Extension Identity (`manifest.json`)**: Embeds a fixed RSA public `"key"` deriving constant Chromium Extension ID `kalfiofhhhcdnnkjehidagebfikodflp` across unpacked developer installs (`chrome://extensions`, `edge://extensions`, `brave://extensions`). Firefox binds extension ID `octoheft@antigravity.dev`.
- **Permissions**: Requests `downloads`, `webRequest`, `nativeMessaging`, `storage`, `tabs`, `activeTab`, `cookies`, and host access `<all_urls>`.
- **Automatic Download Interception (`background.js`)**:
  - Maintains a 53-token static baseline (`DEFAULT_IDM_EXTENSIONS`) and dynamically syncs the live `SettingsStore` list (54 default tokens including `DAT`, plus user wildcards like `R0*`, `R1*`) via Native Messaging `get_settings` on startup, install, tab activation, and download creation, caching in `chrome.storage.local`.
  - Hooks `chrome.downloads.onDeterminingFilename`: ignores non-HTTP schemes (`blob:`, `data:`, `file:`) and extension-self transfers, checks both resolved filename and URL path extension against `activeExactExtensions` + `activeWildcardRegexes`, cancels and erases the native browser download (`chrome.downloads.cancel` + `erase`), collects scoped cookies via `chrome.cookies.getAll`, and dispatches `{ action: "download", ... }` to `com.antigravity.octoheft`.
- **Generic Stream Sniffer & Floating Page Overlay (`background.js`, `content.js`)**:
  - `chrome.webRequest.onHeadersReceived` captures non-YouTube HTTP streams matching `MEDIA_MIMES` or `MEDIA_EXT_REGEX` (`.mp4`, `.mkv`, `.webm`, `.m3u8`, `.mpd`, etc.) above `512 KiB` or streaming manifests, populating the per-tab popup list (`GET_TAB_MEDIA`). Explicitly skips YouTube/`googlevideo.com` URLs to eliminate segmented MSE noise.
  - `content.js` injects an anchored `Download Video` floating button inside `#movie_player`, `#shorts-player`, or `.html5-video-player` using `HIDE_GRACE_MS = 800`, lock-on-hover protection, `MutationObserver`, and `yt-navigate-finish` SPA listener. Clicking sends `GRAB_VIDEO` (`canonicalizeGrabUrl` + `gatherCookieContext`) directly to `OctoMedia` rather than parsing obfuscated player scripts in JS.

### 4.2 Dual Stdio & Windows Named Pipe IPC Bridge (`native_host.rs`)

```text
+----------------+   sendNativeMessage()   +----------------------------------+
| MV3 Extension  | ----------------------> | OctoHeft.exe (Native Host Mode)  |
| background.js  | <---------------------- |  - read_native_frame (32-bit LE) |
+----------------+   stdout JSON response  |  - MAX_INBOUND = 16 MiB          |
                                           |  - MAX_OUTBOUND = 1 MiB          |
                                           +----------------+-----------------+
                                                            |
                                          Line-delimited JSON over Named Pipe:
                                          \\.\pipe\OctoHeftNativeIpc
                                                            |
                                           +----------------v-----------------+
                                           | Running OctoHeft GUI Daemon      |
     If pipe missing:                      |  - ServerOptions::new()          |
     CreateProcessW(DETACHED_PROCESS       |    .first_pipe_instance(true)    |
       | CREATE_NEW_PROCESS_GROUP,         |  - Handles: ping, get_settings,  |
       bInheritHandles = 0)                |    grab_video, download,         |
     Retry 16x250ms (grab) / 8x250ms (dl)  |    download_media                |
     Fallback -> queue_download_offline()  +----------------------------------+
```

**Critical Handle Inheritance Design (`spawn_detached_gui`)**: On Windows, Chromium's Native Messaging host monitor waits until all copies of its redirected `stdout` and `stderr` pipe write-handles close before considering a `sendNativeMessage()` call complete. Passing `bInheritHandles = 0` alongside `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` to `CreateProcessW` ensures a newly spawned GUI + WebView2 instance does **not** inherit Chromium's stdio pipe handles, preventing infinite extension hangs after the short-lived Native Host helper exits.

### 4.3 Canonical Wire Message Schemas

Every wire payload serialized by `background.js` uses canonical snake_case JSON (`NativeMessageRequest` also accepts camelCase aliases via `serde(alias = ...)`):

**1. Extension Handshake & Extension List Sync (`get_settings` / `ping`)**
```json
// Request (Extension -> Native Host -> Pipe -> GUI / Offline SettingsStore)
{ "action": "get_settings" }

// Response
{
  "status": "ok",
  "delivered": "live-ipc-settings",
  "version": "0.1.0",
  "host": "OctoHeft",
  "interceptExtensions": "3GP 7Z AAC ... DAT ... ZIP PKG",
  "defaultDownloadDir": "C:\\Users\\<User>\\Downloads"
}
```

**2. Intercepted Browser Download or Sniffed Stream (`download` / `download_media`)**
```json
// Request
{
  "action": "download",
  "url": "https://cdn.example.org/releases/distro-24.04.iso?token=xyz",
  "file_name": "distro-24.04.iso",
  "title": "distro-24.04",
  "cookies": "session=abc123",
  "referrer": "https://example.org/downloads",
  "user_agent": "Mozilla/5.0 ...",
  "headers": {
    "User-Agent": "Mozilla/5.0 ...",
    "Referer": "https://example.org/downloads",
    "Origin": "https://example.org",
    "Cookie": "session=abc123"
  },
  "mime": "application/x-iso9660-image",
  "total_bytes": 4892106752
}

// Response
{
  "status": "success",
  "taskId": "8f1d4c9a-...",
  "fileName": "distro-24.04.iso",
  "delivered": "live-ipc"
}
```
When split `video_url` and `audio_url` fields are supplied, `handle_live_pipe_request` strips YouTube streaming buffer params (`range`, `rn`, `rbuf`), infers optimal output container (`.mp4` / `.webm` / `.mkv`) from codec/mime/`itag` hints, and routes directly into `dispatch_split_media_task` -> `MediaMuxer::download_and_remux_split_streams` (`"delivered": "live-ipc-mux"`).

**3. Adaptive Video Page Handoff (`grab_video`)**
```json
// Request
{
  "action": "grab_video",
  "url": "https://www.youtube.com/watch?v=...",
  "title": "Video Title",
  "cookies": "...",
  "cookie_entries": [
    {
      "domain": ".youtube.com",
      "path": "/",
      "secure": true,
      "expires": 1799000000,
      "http_only": true,
      "name": "__Secure-1PSID",
      "value": "..."
    }
  ],
  "referrer": "https://www.youtube.com/watch?v=...",
  "user_agent": "Mozilla/5.0 ..."
}

// Response
{
  "status": "success",
  "fileName": "Video Title",
  "delivered": "live-ipc-grab"
}
```
Upon receipt, the GUI stores the payload inside `PendingVideoGrab` (preventing race loss if WebView2 is still booting), restores/focuses the window, and emits `"open-video-grabber"` to switch into the `OctoMedia` view and analyze available formats immediately.

### 4.4 Cookie Extraction & Redirect Security Policy

1. **Full Cookie Jar Fidelity (`gatherCookieContext` -> `omniget/cookies.rs`)**: For authenticated YouTube/member streams, `background.js` queries `chrome.cookies.getAll` across the target page, canonical root, and `accounts.google.com` using the active tab's `cookieStoreId` (isolating incognito/container sessions). `TempCookieFile` writes compliant Netscape `#HttpOnly_` entries while scrubbing `\r`/`\n`/`\t` control characters to prevent line injection and deletes the temporary jar in `Drop`.
2. **Cross-Host Redirect Cookie Strip (`chunk_worker.rs`)**: During manual redirect resolution (`301`–`308`), `should_forward_cookie_on_redirect(from, to)` verifies host equality or subdomain ownership (plus the trusted YouTube -> `googlevideo.com` CDN pair). If an origin redirects to an unrelated external CDN domain, `Cookie` is stripped prior to sending the redirected request.

---

## 5. Technical Specifications & Benchmarks

### 5.1 Measured Build Artifacts & Binary Footprint

Measured directly from the compiled x64 Windows release output (`C:\Users\Twix\Desktop\ree\src-tauri\target\release`):

| Artifact | File Path | Measured Size (Bytes) | Human-Readable Size | Build Configuration |
| :--- | :--- | ---: | ---: | :--- |
| **Dual-Role Application Binary** | `target\release\OctoHeft.exe` | `6,003,200` | `5.73 MiB` | `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`, Common-Controls v6 resource linking (`build.rs`) |
| **NSIS Windows Setup Installer** | `target\release\bundle\nsis\OctoHeft_0.1.0_x64-setup.exe` | `2,624,896` | `2.50 MiB` | Solid LZMA compression (`SetCompressor /SOLID "lzma"`), WebView2 silent `downloadBootstrapper`, 7 installer languages |

### 5.2 Concurrency, Buffer & IPC Specifications

| Subsystem | Operational Parameter | Verified Value / Limit | Source Reference |
| :--- | :--- | :--- | :--- |
| **HTTP Client Pool** | Idle connections per host / connect timeout | `32` sockets/host / `20 s` | `download_engine.rs` |
| **Segment Splitting** | Minimum size threshold / max concurrent connections | `524,288` bytes (`512 KiB`) / `1–32` threads (default `8`) | `download_engine.rs` |
| **Chunk Retry & Redirect** | Max redirect hops / max transient retries / backoff | `6` hops / `5` retries / `300 * 2^retry` ms | `chunk_worker.rs` |
| **Token Bucket Limiter** | Burst capacity floor | `max(rate * 2.0, 131,072)` bytes (`128 KiB`) | `rate_limiter.rs` |
| **Integrity Hashing** | Buffered I/O & single-pass stack chunk size | `128 KiB` `BufReader` / `64 KiB` stack buffer | `post_process.rs` |
| **Remote ZIP Inspector** | Trailing EOCD byte-range tail window | `min(total_size, 65,536)` bytes (`64 KiB`) | `zip_preview.rs` |
| **Batch Expander** | Maximum URL expansion limit per batch | `1,000` URLs | `batch.rs` |
| **Native Messaging Frames** | Max inbound / max outbound JSON payload | `16 MiB` (`16,777,216` B) / `1 MiB` (`1,048,576` B) | `native_host.rs` |
| **Clipboard Monitor** | Poll interval / lock retries / max read size | `500 ms` / `5` retries (`12 ms`) / `65,536` bytes | `clipboard_monitor.rs`, `shell_integration.rs` |
| **UI & Background Cadence** | Progress IPC throttle / Scheduler / Disk telemetry | `150 ms` HTTP, `160 ms` OctoMedia / `1 s` queue / `15 s` active disk | `download_engine.rs`, `downloader.rs`, `scheduler.rs`, `App.tsx` |
| **Omniget Metadata** | Extraction timeout / progress delta | `45 s` hard timeout / `--progress-delta 0.25`, `-N 8` | `extractor.rs`, `downloader.rs` |
| **Exposed Backend API** | Registered Tauri `invoke_handler` commands | `66` commands | `lib.rs` |

### 5.3 Complete Dependency & Toolchain Audit

#### Backend Rust Crates (`src-tauri/Cargo.toml`)

| Crate | Version | Enabled Features / Role |
| :--- | :--- | :--- |
| `tauri` | `2` | `tray-icon` — Desktop application host, WebView2 shell, IPC commands/events, tray builder |
| `tauri-build` | `2` | Build-time resource compilation and Windows Common-Controls v6 manifest integration |
| `tokio` | `1` | `full` — Asynchronous I/O runtime, timers, channels, Named Pipe IPC, subprocess orchestration |
| `reqwest` | `0.12` | `rustls-tls`, `stream`, `json`, `socks` (default-features disabled) — TLS & HTTP/2/SOCKS5 transfer engine |
| `serde` / `serde_json` | `1.0` | `derive` — Zero-copy JSON persistence, Native Messaging wire protocol, `yt-dlp`/`ffprobe` parsing |
| `futures-util` | `0.3` | Async byte-stream combinators (`StreamExt`) for chunk workers and media stream readers |
| `uuid` | `1.10` | `v4`, `serde` — Collision-free task IDs and ephemeral cookie/temp filenames |
| `chrono` | `0.4` | `serde` — UTC task timestamps and localized queue day/time schedule evaluation |
| `parking_lot` | `0.12` | Compact, fair `RwLock`/`Mutex` guards for in-memory engine & state caches |
| `url` | `2.5` | RFC 3986 URI parsing, path/query isolation, signed filename extraction, redirect host checks |
| `bytes` | `1.7` | Reference-counted zero-copy sample buffers in `media_muxer.rs` |
| `sha2` / `md-5` / `hex` | `0.10` / `0.10` / `0.4` | File checksum verification, Omniget release binary SHA-256 verification |
| `flate2` | `1.0` | Deflate stream decompression in `zip_preview.rs` |
| `mp4` | `0.14` | Pure-Rust ISOBMFF container reading/writing (`ftyp`/`moov`/`mdat`) in `media_muxer.rs` |
| `winreg` | `0.56` | Typed HKCU Registry management (Startup `Run`, URL protocol, context menu, Native Messaging) |
| `tracing` / `tracing-subscriber` / `tracing-appender` | `0.1` / `0.3` / `0.2` | `std`, `fmt` — Structured diagnostic logging to rotating disk logs and UI viewer |
| `thiserror` | `2.0` | Typed backend `EngineError` propagation |
| `windows-sys` | `0.59` | 16 Win32/COM/Shell feature modules (`Storage_FileSystem`, `System_Ole`, `UI_Shell`, `UI_Shell_Common`, `System_DataExchange`, `System_Memory`, `System_Threading`, `Graphics_Gdi`, `Globalization`, etc.) |

#### Frontend NPM Dependencies (`package.json`)

| Package | Version | Scope | Role |
| :--- | :--- | :--- | :--- |
| `react` / `react-dom` | `^18.3.1` | Runtime | Component UI tree and ReactDOM portal rendering (`Header` speed popover) |
| `@tauri-apps/api` | `^2.2.0` | Runtime | Frontend IPC `invoke`, event listeners, window management |
| `@tauri-apps/plugin-shell` | `^2.2.0` | Runtime | Controlled shell interactions |
| `lucide-react` | `^0.475.0` | Runtime | Crisp vector UI iconography |
| `clsx` / `tailwind-merge` | `^2.1.1` / `^2.6.0` | Runtime | Class composition utilities |
| `vite` / `@vitejs/plugin-react` | `^6.1.0` / `^4.3.4` | Dev | ES module bundler and fast HMR |
| `typescript` | `^5.7.3` | Dev | Strict static type checking (`tsc && vite build`) |
| `tailwindcss` / `postcss` / `autoprefixer` | `^3.4.17` / `^8.5.2` / `^10.4.20` | Dev | Utility-first styling with custom `neon` variant |
| `@tauri-apps/cli` | `^2.2.4` | Dev | Native compilation and NSIS bundle packaging |

### 5.4 Hybrid Runtime Dependency Matrix

OctoHeft separates zero-dependency native execution from optional/managed third-party extractor utilities:

| Feature / Workflow | Execution Runtime | External Binary Required? | Verification / Fallback Behavior |
| :--- | :--- | :--- | :--- |
| Segmented HTTP/HTTPS Transfers (<= 32 streams) | Pure Rust (`reqwest` + `SparseFile`) | **No** | Self-contained inside `OctoHeft.exe` |
| Remote ZIP Preview & Selective Extract | Pure Rust (`zip_preview.rs` + `flate2`) | **No** | Self-contained via HTTP 206 byte ranges |
| Direct HLS (`.m3u8`), DASH (`.mpd`) & Split MSE Muxing | Pure Rust (`media_muxer.rs` + `mp4`) | **No** | Zero external FFmpeg needed for HLS/DASH/MSE |
| Browser Native Messaging & Named Pipe IPC | Pure Rust (`native_host.rs` + `tokio`) | **No** | `OctoHeft.exe` serves directly as Chrome/Firefox host |
| Win32 OLE Drag-Drop, Clipboard, Tray, Defender, Hash | Pure Rust (`windows-sys`, `sha2`, `md-5`) | **No** | Defender uses inbox Windows `MpCmdRun.exe` when present |
| OctoMedia Multi-Site Extraction & 4K Format Curation | `omniget/` + managed `yt-dlp.exe` & `deno.exe` (opt.) | **Auto-Managed / Optional** | Discovers Winget/Scoop/PATH or auto-downloads SHA-256-verified binaries; falls back to direct HTTP media probe if absent |
| OctoMedia Adaptive Lossless Remux & Stream Audit | `omniget/` + managed `ffmpeg.exe` & `ffprobe.exe` | **Auto-Managed / Optional** | Downloaded via SHA-256-verified Gyan essentials archive on one-click `Auto-Setup Tools` |

### 5.5 Internationalization, NSIS Packaging & Registry Footprint

- **7-Language Real-Time i18n & RTL Engine (`src/i18n/index.tsx`, `tray.rs`)**: Ships complete translation dictionaries (`en`, `fr`, `es`, `de`, `ar`, `ru`, `zh`) synchronized immediately across `<html lang dir>`, frontend UI modals, number/speed formatting, `<progress>` animation direction, and native Win32 tray menu items. Arabic (`ar`) activates full `dir="rtl"` mirroring across both the NSIS installer (`nsDialogs::SetRTL 1`) and the Tauri WebView2 UI.
- **4-Tier Locale Detection Cascade (`settings.rs`)**:
  1. One-shot NSIS setup selection (`HKCU\Software\OctoHeft\InstallerLanguageOverride`, consumed and deleted on first launch).
  2. Persisted user choice (`settings_state.json` -> `language`).
  3. Installed language fallback (`HKCU\Software\OctoHeft\Language`).
  4. Win32 OS locale query (`GetUserDefaultLocaleName`).
- **NSIS Windows Registry & Shell Hooks (`src-tauri/nsis/hooks.nsh`, `installer.nsi`)**:
  - **Custom URI Scheme**: `HKCU\Software\Classes\octoheft` -> `"<INSTDIR>\OctoHeft.exe" "%1"`.
  - **Queue File Association**: `.octodl` (`OctoHeft Queue File`).
  - **Explorer Context Menu**: `HKCU\Software\Classes\*\shell\OctoHeftChecksum` (`Verify checksum with OctoHeft`) -> `"<INSTDIR>\OctoHeft.exe" --verify "%1"`.
  - **Native Messaging Registration**: Generates `com.antigravity.octoheft.json` (Chrome/Edge/Brave) and `com.antigravity.octoheft.firefox.json` inside `$INSTDIR\resources\` and registers keys under `HKCU\Software\{Google\Chrome,Microsoft\Edge,BraveSoftware\Brave-Browser,Mozilla}\NativeMessagingHosts\com.antigravity.octoheft`.
  - **Finish-Page Activation Wizard**: Checked by default; launches detected `chrome.exe` / `msedge.exe` / `brave.exe` directly to `<browser>://extensions` via `nsis_tauri_utils::RunAsUser`.
  - **Clean Uninstall (`NSIS_HOOK_POSTUNINSTALL`)**: Removes all `HKCU` integration keys, manifests, extension assets, startup entries, and (when requested outside updates) `%APPDATA%\octoheft` and `%LOCALAPPDATA%\OctoHeft`.

---

## 6. Demonstration Video Script & Walkthrough Guide

This structured capture script provides a repeatable sequence to verify and showcase OctoHeft's end-to-end engineering capabilities cleanly.

### Demo Intro: Architecture & Genesis (OctoHeft vs. Legacy IDM)
- **Visual Shot**: Side-by-side property sheet showing `OctoHeft.exe` (`6,003,200` bytes) and `OctoHeft_0.1.0_x64-setup.exe` (`2,624,896` bytes), transitioning to OctoHeft's open desktop window at `125%`–`150%` Windows 11 fractional scaling.
- **Narration / On-Screen Callout**:
  - Highlight single monolithic dual-role binary architecture (zero separate `IDMMsgHost.exe` daemons) and sharp `PerMonitorV2` WebView2 Fluent typography (`Segoe UI Variable`) contrasted with legacy fixed-DPI bitmap skinning.

### Segment 1: Segmented Acceleration, 0 ms Sparse Completion & Core Utilities
- **Visual Shot**:
  1. Click `+ Add URL` (`Header.tsx`), paste a multi-gigabyte Linux ISO URL, adjust **Parallel Streams** slider from `8 (Optimal)` up to `16 (Turbo)`/`32 (Max)`, and click `Add Download`.
  2. Inspect the live row in `DownloadTable.tsx` and open `DownloadDetailModal` -> `Transfer & Segments` tab showing live speed graph (`30s ago` -> `Now`, peak speed readout) and individual `Conn #0..N` byte-range bars.
  3. Capture progress from `98%` -> `100% Completed`: point out instantaneous `Completed` transition with zero disk freeze or temporary part concatenation.
  4. Open header utilities: **ZIP Preview** (`Inspect Remote ZIP` on a large online archive showing only `64 KB` transferred, selective single-entry `Extract File`), **Batch** (`https://.../chapter_[001-050].pdf` live preview count), and **Scheduler** (concurrency cap, active weekday buttons `Sun`–`Sat`, post-action `Sleep Computer` / `Shutdown PC`).
- **Expected Result**: All `32` connections write concurrently to one sparse NTFS file (`FSCTL_SET_SPARSE`), completing immediately; remote ZIP browsing reads metadata without downloading the archive.

### Segment 2: Browser Extension Capture & Clipboard Monitor Focus Restoration
- **Visual Shot**:
  1. Click the **Clipboard Monitor** icon button in the header utility pill (`data-clipboard-monitor="active"`, cyan LED badge illuminates). Minimize OctoHeft to the Windows desktop or switch focus into Chrome/Edge.
  2. Copy a signed archive link containing query parameters (e.g., `https://cdn.example.com/archive.rar?auth=token123`) to the Windows clipboard.
  3. Watch OctoHeft restore and jump above active windows via DWM focus pulse, opening `Add New Download` pre-filled with `archive.rar` and the **`Captured from Clipboard`** badge.
  4. Switch to Chrome (`chrome://extensions` showing `OctoHeft Integration Module v1.1.0`, ID `kalfiofhhhcdnnkjehidagebfikodflp`), click a `.zip` or `.exe` download link to trigger `onDeterminingFilename` interception (download is canceled from Chrome history and routed via `OctoHeft.exe` stdio -> `\\.\pipe\OctoHeftNativeIpc` directly into OctoHeft's queue).
- **Expected Result**: Sub-second clipboard pickup even behind foreground browsers and silent native handoff without stalled Chrome process handles.

### Segment 3: OctoMedia 4K Extraction, Container Curation & Live Cancel/Reset
- **Visual Shot**:
  1. Open a 4K video in Chrome/Edge; hover over the player to reveal the floating `Download Video` pill (`HIDE_GRACE_MS = 800`), click it (`Opening OctoMedia...` -> `Opened in OctoMedia`), and confirm OctoHeft switches to **OctoMedia**, displays the `Engine ready` status badge and **`Browser cookies attached for protected/member streams`** pill.
  2. Inspect curated quality options (`4K 2160p` -> `.mkv` / `.webm`, `1080p` -> `.mp4`, `Audio Only` -> `.m4a`/`Opus`).
  3. Test state hygiene: click **`Cancel / Change video`** (or the `X` **`Clear URL`** button inside the input), switch away to **Downloads Queue** in the sidebar and back to **OctoMedia** to confirm the cleared URL does not reload unwanted analysis.
  4. Re-fetch, click `Download Selected Quality`, switch to `DownloadDetailModal` -> `Checksum & Defender Scan` & `Media Stream & Remuxer` tabs showing `-N 8` progress, lossless FFmpeg merge, and post-mux `FFprobe` verification.
- **Expected Result**: Clean 4K video+audio file playable in Windows Media Player / VLC with zero HTTP 403 failures and persistent tab state when navigating views.

### Segment 4: Outbound Win32 OLE Drag-and-Drop & Checksum Verification
- **Visual Shot**:
  1. Open an empty Windows File Explorer folder next to OctoHeft.
  2. Click-and-drag directly on a **Completed** row (or its category icon) in `DownloadTable.tsx` out onto the Desktop or into the Explorer window (cursor becomes `grabbing`, Windows native shell copy tooltip appears, file drops into Explorer via `SHDoDragDrop` / `CF_HDROP`).
  3. Also drag using only the left-column grip icon (`GripVertical`) inside the table to demonstrate that queue priority reordering still works independently inside the app.
  4. Open the completed task's Inspector (`Checksum & Defender Scan`), click **`Compute Hashes`** (single-pass SHA-256 + MD5) and **`Scan with Defender`** (`MpCmdRun.exe`), and right-click the dropped file in Windows Explorer showing **`Verify checksum with OctoHeft`**.
- **Expected Result**: Full native Explorer interoperability out of a WebView2 React UI without HTML5 data-URI workarounds.

### Segment 5: 3-Way Theme Engine, Live Disk Telemetry, Tray Lifecycle & 7-Language RTL
- **Visual Shot**:
  1. Click the **Theme** icon adjacent to Settings in the right-hand utility pill three times to demonstrate **`Light` (Sun) -> `Dark` (Moon) -> `Neon` Cyber Navy (Zap `#00122e` / `#00f0ff`)**.
  2. Resize the OctoHeft window narrower and wider to show the CSS `@container` `subgrid` toolbar collapsing cleanly onto two rows and the Download Queue table transforming into stacked cards below `780px` without horizontal overflow.
  3. Point to the bottom-left **Storage Status** card (`Drive C: ... free` `<progress>` bar + live `Session:` counter).
  4. Click the window close button `[X]` (window hides cleanly to the notification tray); single left-click the OctoHeft tray icon to prove instant, flicker-free restoration, then right-click the tray icon to inspect **`Start with Windows`**. Press **`Ctrl+Alt+D`** globally to open Quick Paste.
  5. Open **Settings** (`General` / `File Types` / `Save To` / `Browser Integration`), demonstrate `Restore Defaults` (54 intercepted tokens), switch **Display Language** from `English` to **`العربية (Arabic)`** showing instantaneous full-UI and tray-menu `RTL` layout mirroring, then return to `English`.
- **Expected Result**: Responsive, leak-free desktop integration across DPIs, themes, languages, and Windows lifecycle states.
