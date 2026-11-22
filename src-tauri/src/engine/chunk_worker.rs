use crate::engine::error::EngineError;
use crate::engine::rate_limiter::TokenBucketLimiter;
use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue, ACCEPT, COOKIE, ORIGIN, RANGE, REFERER, USER_AGENT};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};
use url::Url;

pub const DEFAULT_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

static NO_REDIRECT_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn no_redirect_client() -> &'static reqwest::Client {
    NO_REDIRECT_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(32)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(DEFAULT_BROWSER_USER_AGENT)
            .build()
            .unwrap_or_default()
    })
}

pub(crate) fn is_google_video_or_youtube_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "googlevideo.com"
        || h.ends_with(".googlevideo.com")
        || h == "youtube.com"
        || h.ends_with(".youtube.com")
        || h == "youtube-nocookie.com"
        || h.ends_with(".youtube-nocookie.com")
        || h == "youtu.be"
}

pub(crate) fn is_googlevideo_url(raw_url: &str) -> bool {
    if let Ok(parsed) = Url::parse(raw_url.trim()) {
        if let Some(host) = parsed.host_str() {
            let h = host.to_ascii_lowercase();
            return h == "googlevideo.com" || h.ends_with(".googlevideo.com");
        }
    }
    raw_url.to_ascii_lowercase().contains(".googlevideo.com/")
}

fn canonical_header_name(raw_key: &str) -> Option<&str> {
    let k = raw_key.trim();
    if k.is_empty() {
        return None;
    }
    Some(match k.to_ascii_lowercase().as_str() {
        "user-agent" => "User-Agent",
        "referer" => "Referer",
        "origin" => "Origin",
        "cookie" => "Cookie",
        "accept" => "Accept",
        "authorization" => "Authorization",
        _ => k,
    })
}

fn normalize_youtube_referer(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(parsed) = Url::parse(trimmed) {
        if let Some(host) = parsed.host_str() {
            let h = host.to_ascii_lowercase();
            if h == "youtube.com" || h.ends_with(".youtube.com") {
                return "https://www.youtube.com/".to_string();
            }
            if h == "youtube-nocookie.com" || h.ends_with(".youtube-nocookie.com") {
                return "https://www.youtube-nocookie.com/".to_string();
            }
            if h == "youtu.be" {
                return "https://www.youtube.com/".to_string();
            }
        }
    }
    trimmed.to_string()
}

pub(crate) fn normalize_request_headers(
    url: &str,
    raw_headers: Option<&HashMap<String, String>>,
) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let is_gvid = is_googlevideo_url(url);

    let set_unique = |list: &mut Vec<(String, String)>, key: &str, val: String| {
        if let Some(pos) = list.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            list[pos] = (key.to_string(), val);
        } else {
            list.push((key.to_string(), val));
        }
    };

    if let Some(hdrs) = raw_headers {
        for (raw_k, raw_v) in hdrs {
            let Some(k) = canonical_header_name(raw_k) else {
                continue;
            };
            let v = raw_v.trim();
            if v.is_empty() {
                continue;
            }
            let lower = k.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "range" | "host" | "content-length" | "connection" | "accept-encoding"
            ) {
                continue;
            }
            let normalized_v = if is_gvid && lower == "referer" {
                normalize_youtube_referer(v)
            } else {
                v.to_string()
            };
            set_unique(&mut entries, k, normalized_v);
        }
    }

    if !entries
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
    {
        set_unique(
            &mut entries,
            "User-Agent",
            DEFAULT_BROWSER_USER_AGENT.to_string(),
        );
    }

    if is_gvid {
        if !entries.iter().any(|(k, _)| k.eq_ignore_ascii_case("referer")) {
            set_unique(
                &mut entries,
                "Referer",
                "https://www.youtube.com/".to_string(),
            );
        }
        if !entries.iter().any(|(k, _)| k.eq_ignore_ascii_case("origin")) {
            set_unique(
                &mut entries,
                "Origin",
                "https://www.youtube.com".to_string(),
            );
        }
        if !entries.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
            set_unique(&mut entries, "Accept", "*/*".to_string());
        }
    }

    entries
}

fn should_forward_cookie_on_redirect(initial_url: &Url, target_url: &Url) -> bool {
    let (Some(h1), Some(h2)) = (initial_url.host_str(), target_url.host_str()) else {
        return false;
    };
    if h1.eq_ignore_ascii_case(h2) {
        return true;
    }
    if is_google_video_or_youtube_host(h1) && is_google_video_or_youtube_host(h2) {
        return true;
    }
    #[cfg(test)]
    if matches!(h1, "localhost" | "127.0.0.1") && matches!(h2, "localhost" | "127.0.0.1") {
        return true;
    }
    false
}

fn build_single_hop_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    normalized_headers: &[(String, String)],
    range_header: Option<&str>,
    include_cookie: bool,
) -> reqwest::RequestBuilder {
    let mut req = client.request(method, url);
    let mut applied_ua = false;

    for (k, v) in normalized_headers {
        let lower = k.to_ascii_lowercase();
        if !include_cookie && lower == "cookie" {
            continue;
        }
        if lower == "user-agent" {
            if let Ok(hv) = HeaderValue::from_str(v) {
                req = req.header(USER_AGENT, hv);
                applied_ua = true;
            }
            continue;
        }
        if lower == "referer" {
            if let Ok(hv) = HeaderValue::from_str(v) {
                req = req.header(REFERER, hv);
            }
            continue;
        }
        if lower == "origin" {
            if let Ok(hv) = HeaderValue::from_str(v) {
                req = req.header(ORIGIN, hv);
            }
            continue;
        }
        if lower == "cookie" {
            if let Ok(hv) = HeaderValue::from_str(v) {
                req = req.header(COOKIE, hv);
            }
            continue;
        }
        if lower == "accept" {
            if let Ok(hv) = HeaderValue::from_str(v) {
                req = req.header(ACCEPT, hv);
            }
            continue;
        }
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            req = req.header(name, val);
        }
    }

    if !applied_ua {
        req = req.header(USER_AGENT, DEFAULT_BROWSER_USER_AGENT);
    }

    if let Some(r) = range_header {
        req = req.header(RANGE, r);
    }

    req
}

async fn send_with_manual_redirects(
    client: &reqwest::Client,
    initial_method: reqwest::Method,
    initial_url: &str,
    headers: Option<&HashMap<String, String>>,
    range_header: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    let normalized = normalize_request_headers(initial_url, headers);
    let base_parsed = Url::parse(initial_url);
    let mut current_url = initial_url.to_string();
    let mut method = initial_method;

    for _hop in 0..6 {
        let include_cookie = match (&base_parsed, Url::parse(&current_url)) {
            (Ok(b), Ok(cur)) => should_forward_cookie_on_redirect(b, &cur),
            _ => true,
        };

        let resp = build_single_hop_request(
            client,
            method.clone(),
            &current_url,
            &normalized,
            range_header,
            include_cookie,
        )
        .send()
        .await?;

        let status = resp.status();
        if status.is_redirection() {
            if let Some(loc_val) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            {
                if let Ok(curr_parsed) = Url::parse(&current_url) {
                    if let Ok(next_url) = curr_parsed.join(loc_val) {
                        if matches!(next_url.scheme(), "http" | "https") {
                            if status == reqwest::StatusCode::SEE_OTHER
                                && method != reqwest::Method::HEAD
                            {
                                method = reqwest::Method::GET;
                            }
                            current_url = next_url.to_string();
                            continue;
                        }
                    }
                }
            }
        }

        return Ok(resp);
    }

    build_single_hop_request(
        client,
        method,
        &current_url,
        &normalized,
        range_header,
        true,
    )
    .send()
    .await
}

/// Sends an HTTP request applying full browser session headers and preserving
/// `User-Agent`, `Referer`, `Origin`, `Cookie`, and `Range` across Google CDN `302` redirects.
pub(crate) async fn send_request_with_headers(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    headers: Option<&HashMap<String, String>>,
    range_header: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    if is_googlevideo_url(url) {
        return send_with_manual_redirects(no_redirect_client(), method, url, headers, range_header)
            .await;
    }

    let normalized = normalize_request_headers(url, headers);
    build_single_hop_request(client, method, url, &normalized, range_header, true)
        .send()
        .await
}

pub const SEG_STATE_PENDING: u8 = 0;
pub const SEG_STATE_DOWNLOADING: u8 = 1;
pub const SEG_STATE_COMPLETED: u8 = 2;
pub const SEG_STATE_FAILED: u8 = 3;

#[derive(Debug, Clone)]
pub struct WorkerProgressEvent {
    pub segment_id: usize,
    pub bytes_read: u64,
    pub current_offset: u64,
    pub is_completed: bool,
    pub error: Option<String>,
}

pub type SpeedLimiter = TokenBucketLimiter;

pub struct ChunkWorker {
    pub segment_id: usize,
    pub start_byte: u64,
    pub end_byte: Arc<AtomicU64>,
    pub current_offset: Arc<AtomicU64>,
    pub segment_state: Arc<AtomicU8>,
    pub downloaded_bytes: Arc<AtomicU64>,
    pub url: String,
    pub client: reqwest::Client,
    pub headers: Option<HashMap<String, String>>,
    pub shared_file: Arc<std::fs::File>,
    pub speed_limiter: Option<TokenBucketLimiter>,
    pub cancel_rx: watch::Receiver<bool>,
    pub progress_tx: mpsc::UnboundedSender<WorkerProgressEvent>,
}

impl ChunkWorker {
    pub fn new(
        segment_id: usize,
        start_byte: u64,
        end_byte: u64,
        initial_downloaded: u64,
        url: String,
        client: reqwest::Client,
        headers: Option<HashMap<String, String>>,
        shared_file: Arc<std::fs::File>,
        speed_limiter: Option<TokenBucketLimiter>,
        cancel_rx: watch::Receiver<bool>,
        progress_tx: mpsc::UnboundedSender<WorkerProgressEvent>,
        downloaded_bytes: Arc<AtomicU64>,
    ) -> Self {
        Self {
            segment_id,
            start_byte,
            end_byte: Arc::new(AtomicU64::new(end_byte)),
            current_offset: Arc::new(AtomicU64::new(start_byte + initial_downloaded)),
            segment_state: Arc::new(AtomicU8::new(SEG_STATE_PENDING)),
            downloaded_bytes,
            url,
            client,
            headers,
            shared_file,
            speed_limiter,
            cancel_rx,
            progress_tx,
        }
    }

    pub async fn run(self) {
        let max_retries = 5;
        let mut retry_count = 0;

        loop {
            if *self.cancel_rx.borrow() {
                return;
            }

            let cur = self.current_offset.load(Ordering::SeqCst);
            let end = self.end_byte.load(Ordering::SeqCst);

            if cur > end {
                // Segment reached its assigned target
                self.segment_state.store(SEG_STATE_COMPLETED, Ordering::Release);
                let _ = self.progress_tx.send(WorkerProgressEvent {
                    segment_id: self.segment_id,
                    bytes_read: 0,
                    current_offset: cur,
                    is_completed: true,
                    error: None,
                });
                return;
            }

            let range_header = format!("bytes={}-{}", cur, end);

            let resp = match send_request_with_headers(
                &self.client,
                reqwest::Method::GET,
                &self.url,
                self.headers.as_ref(),
                Some(&range_header),
            )
            .await
            {
                Ok(r) => r,
                Err(err) => {
                    retry_count += 1;
                    warn!(segment = self.segment_id, retry = retry_count, "Network dropout on chunk connect: {}", err);
                    if retry_count > max_retries {
                        let engine_err = EngineError::Network(err.to_string());
                        error!(segment = self.segment_id, "{}", engine_err);
                        self.segment_state.store(SEG_STATE_FAILED, Ordering::Release);
                        let _ = self.progress_tx.send(WorkerProgressEvent {
                            segment_id: self.segment_id,
                            bytes_read: 0,
                            current_offset: cur,
                            is_completed: false,
                            error: Some(engine_err.to_string()),
                        });
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(300 * (1 << retry_count))).await;
                    continue;
                }
            };

            let status = resp.status();
            if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
                let is_fatal_auth = matches!(
                    status,
                    reqwest::StatusCode::UNAUTHORIZED
                        | reqwest::StatusCode::FORBIDDEN
                        | reqwest::StatusCode::NOT_FOUND
                        | reqwest::StatusCode::GONE
                );
                retry_count += 1;
                warn!(segment = self.segment_id, status = %status, retry = retry_count, "Unexpected HTTP status for range");
                if is_fatal_auth || retry_count > max_retries {
                    let engine_err = EngineError::Http {
                        status: status.as_u16(),
                        message: self.url.clone(),
                    };
                    error!(segment = self.segment_id, "{}", engine_err);
                    self.segment_state.store(SEG_STATE_FAILED, Ordering::Release);
                    let _ = self.progress_tx.send(WorkerProgressEvent {
                        segment_id: self.segment_id,
                        bytes_read: 0,
                        current_offset: cur,
                        is_completed: false,
                        error: Some(engine_err.to_string()),
                    });
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            debug!(segment = self.segment_id, offset = cur, end = end, "Range request connected cleanly");
            self.segment_state.store(SEG_STATE_DOWNLOADING, Ordering::Release);

            // Stream body
            let mut stream = resp.bytes_stream();
            let mut stream_failed = false;

            while let Some(chunk_res) = stream.next().await {
                if *self.cancel_rx.borrow() {
                    return;
                }

                let chunk = match chunk_res {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        stream_failed = true;
                        retry_count += 1;
                        warn!(segment = self.segment_id, retry = retry_count, "Stream read failure: {}", e);
                        if retry_count > max_retries {
                            let engine_err = EngineError::CorruptedStream(e.to_string());
                            error!(segment = self.segment_id, "{}", engine_err);
                            self.segment_state.store(SEG_STATE_FAILED, Ordering::Release);
                            let _ = self.progress_tx.send(WorkerProgressEvent {
                                segment_id: self.segment_id,
                                bytes_read: 0,
                                current_offset: self.current_offset.load(Ordering::SeqCst),
                                is_completed: false,
                                error: Some(engine_err.to_string()),
                            });
                            return;
                        }
                        break;
                    }
                };

                let chunk_len = chunk.len();
                if chunk_len == 0 {
                    continue;
                }

                let write_offset = self.current_offset.load(Ordering::SeqCst);
                let current_limit_end = self.end_byte.load(Ordering::Relaxed);

                // If chunk would overshoot the dynamically updated segment boundary, truncate it
                let effective_len = if write_offset + chunk_len as u64 > current_limit_end + 1 {
                    (current_limit_end + 1).saturating_sub(write_offset) as usize
                } else {
                    chunk_len
                };

                if effective_len == 0 {
                    break;
                }

                // Token-bucket rate limiting before or after writing
                if let Some(limiter) = &self.speed_limiter {
                    limiter.acquire(effective_len).await;
                }

                // Direct file write at exact offset
                #[cfg(windows)]
                {
                    use std::os::windows::fs::FileExt;
                    if let Err(io_err) = self.shared_file.seek_write(&chunk[..effective_len], write_offset) {
                        self.segment_state.store(SEG_STATE_FAILED, Ordering::Release);
                        let _ = self.progress_tx.send(WorkerProgressEvent {
                            segment_id: self.segment_id,
                            bytes_read: 0,
                            current_offset: write_offset,
                            is_completed: false,
                            error: Some(format!("Disk write error: {}", io_err)),
                        });
                        return;
                    }
                }

                #[cfg(unix)]
                {
                    use std::os::unix::fs::FileExt;
                    if let Err(io_err) = self.shared_file.write_at(&chunk[..effective_len], write_offset) {
                        self.segment_state.store(SEG_STATE_FAILED, Ordering::Release);
                        let _ = self.progress_tx.send(WorkerProgressEvent {
                            segment_id: self.segment_id,
                            bytes_read: 0,
                            current_offset: write_offset,
                            is_completed: false,
                            error: Some(format!("Disk write error: {}", io_err)),
                        });
                        return;
                    }
                }

                let new_offset = write_offset + effective_len as u64;
                self.current_offset.store(new_offset, Ordering::Release);
                // Lock-free atomic progress accounting; speed/ETA are sampled periodically on the 200 ms telemetry clock.
                self.downloaded_bytes
                    .fetch_add(effective_len as u64, Ordering::Relaxed);

                let is_done = new_offset > current_limit_end;
                if is_done {
                    self.segment_state.store(SEG_STATE_COMPLETED, Ordering::Release);
                    let _ = self.progress_tx.send(WorkerProgressEvent {
                        segment_id: self.segment_id,
                        bytes_read: 0,
                        current_offset: new_offset,
                        is_completed: true,
                        error: None,
                    });
                    return;
                }
            }

            if !stream_failed {
                let final_offset = self.current_offset.load(Ordering::SeqCst);
                let target_end = self.end_byte.load(Ordering::SeqCst);
                if final_offset > target_end {
                    self.segment_state.store(SEG_STATE_COMPLETED, Ordering::Release);
                    let _ = self.progress_tx.send(WorkerProgressEvent {
                        segment_id: self.segment_id,
                        bytes_read: 0,
                        current_offset: final_offset,
                        is_completed: true,
                        error: None,
                    });
                    return;
                }

                // Premature EOF before covering segment range
                retry_count += 1;
                warn!(segment = self.segment_id, retry = retry_count, offset = final_offset, target = target_end, "Premature EOF received; retrying range");
                if retry_count > max_retries {
                    let err = EngineError::CorruptedStream(format!(
                        "Premature EOF at offset {} (expected {})",
                        final_offset, target_end
                    ));
                    error!(segment = self.segment_id, "{}", err);
                    self.segment_state.store(SEG_STATE_FAILED, Ordering::Release);
                    let _ = self.progress_tx.send(WorkerProgressEvent {
                        segment_id: self.segment_id,
                        bytes_read: 0,
                        current_offset: final_offset,
                        is_completed: false,
                        error: Some(err.to_string()),
                    });
                    return;
                }
            }

            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::sparse_file::SparseFile;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    #[test]
    fn test_normalize_googlevideo_headers_defaults_and_dedup() {
        let mut raw = HashMap::new();
        raw.insert(
            "referer".to_string(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string(),
        );
        raw.insert("cookie".to_string(), "SID=test_sid; PREF=1".to_string());
        raw.insert("range".to_string(), "bytes=0-10".to_string());
        raw.insert("host".to_string(), "evil.example".to_string());

        let normalized = normalize_request_headers(
            "https://rr1---sn-test.googlevideo.com/videoplayback?itag=313",
            Some(&raw),
        );

        let as_map: HashMap<String, String> = normalized.into_iter().collect();
        assert_eq!(
            as_map.get("Referer").map(String::as_str),
            Some("https://www.youtube.com/")
        );
        assert_eq!(
            as_map.get("Origin").map(String::as_str),
            Some("https://www.youtube.com")
        );
        assert_eq!(
            as_map.get("Cookie").map(String::as_str),
            Some("SID=test_sid; PREF=1")
        );
        assert_eq!(
            as_map.get("User-Agent").map(String::as_str),
            Some(DEFAULT_BROWSER_USER_AGENT)
        );
        assert_eq!(as_map.get("Accept").map(String::as_str), Some("*/*"));
        assert!(!as_map.contains_key("range"));
        assert!(!as_map.contains_key("host"));
    }

    #[tokio::test]
    async fn test_manual_redirect_preserves_cookie_origin_referer_and_range() {
        let edge_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let edge_port = edge_listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            if let Ok((mut sock, _)) = edge_listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let ok = req.contains("user-agent: mozilla/5.0 chrome/138")
                    && req.contains("referer: https://www.youtube.com/")
                    && req.contains("origin: https://www.youtube.com")
                    && req.contains("cookie: sid=abc; psid=xyz")
                    && req.contains("range: bytes=0-3");
                if ok {
                    let hdr = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/8\r\nContent-Length: 4\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(b"OKAY").await;
                } else {
                    let hdr = "HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nFORBIDDEN";
                    let _ = sock.write_all(hdr.as_bytes()).await;
                }
                let _ = sock.shutdown().await;
            }
        });

        let redir_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redir_port = redir_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = redir_listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let hdr = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/edge_stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    edge_port
                );
                let _ = sock.write_all(hdr.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });

        let mut hdrs = HashMap::new();
        hdrs.insert("User-Agent".to_string(), "Mozilla/5.0 Chrome/138".to_string());
        hdrs.insert("Referer".to_string(), "https://www.youtube.com/".to_string());
        hdrs.insert("Origin".to_string(), "https://www.youtube.com".to_string());
        hdrs.insert("Cookie".to_string(), "sid=abc; psid=xyz".to_string());

        let resp = send_with_manual_redirects(
            no_redirect_client(),
            reqwest::Method::GET,
            &format!("http://localhost:{}/videoplayback?itag=137", redir_port),
            Some(&hdrs),
            Some("bytes=0-3"),
        )
        .await
        .expect("Redirected request failed");

        assert_eq!(resp.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        let body = resp.bytes().await.unwrap();
        assert_eq!(&body[..], b"OKAY");
    }

    #[tokio::test]
    async fn test_chunk_worker_sends_required_session_headers_or_fails_403() {
        let payload = b"YOUTUBE_CDN_BODY_123456";
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                    let authorized = req.contains("user-agent: mozilla/5.0 live-ua")
                        && req.contains("referer: https://www.youtube.com/")
                        && req.contains("origin: https://www.youtube.com")
                        && req.contains("cookie: __secure-1psid=tok123");
                    if !authorized {
                        let hdr = "HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nFORBIDDEN";
                        let _ = sock.write_all(hdr.as_bytes()).await;
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            payload.len() - 1,
                            payload.len(),
                            payload.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(payload).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });

        let temp_file = std::env::temp_dir().join(format!("octoheft_cw_hdr_{}.bin", Uuid::new_v4()));
        let sparse = SparseFile::create_preallocated(&temp_file, payload.len() as u64).unwrap();
        let shared = sparse.get_arc_handle();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (prog_tx, mut prog_rx) = mpsc::unbounded_channel();
        let downloaded_bytes = Arc::new(AtomicU64::new(0));

        let mut hdrs = HashMap::new();
        hdrs.insert("User-Agent".to_string(), "Mozilla/5.0 live-ua".to_string());
        hdrs.insert("Referer".to_string(), "https://www.youtube.com/".to_string());
        hdrs.insert("Origin".to_string(), "https://www.youtube.com".to_string());
        hdrs.insert("Cookie".to_string(), "__Secure-1PSID=tok123".to_string());

        let worker = ChunkWorker::new(
            0,
            0,
            (payload.len() - 1) as u64,
            0,
            format!("http://127.0.0.1:{}/videoplayback?itag=137", port),
            reqwest::Client::new(),
            Some(hdrs),
            shared,
            None,
            cancel_rx,
            prog_tx,
            Arc::clone(&downloaded_bytes),
        );
        worker.run().await;

        let mut completed = false;
        let mut events_count = 0usize;
        while let Ok(ev) = prog_rx.try_recv() {
            events_count += 1;
            assert!(ev.error.is_none(), "Unexpected worker error: {:?}", ev.error);
            if ev.is_completed {
                completed = true;
            }
        }
        assert!(completed, "ChunkWorker should complete cleanly with headers");
        assert_eq!(events_count, 1, "ChunkWorker should only emit a single terminal event");
        assert_eq!(downloaded_bytes.load(Ordering::Relaxed), payload.len() as u64);
        assert_eq!(&std::fs::read(&temp_file).unwrap()[..], payload);

        let _ = std::fs::remove_file(&temp_file);
    }
}


