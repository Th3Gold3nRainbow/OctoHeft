use octoheft_engine_lib::engine::sparse_file::SparseFile;
use octoheft_engine_lib::engine::{
    AddDownloadRequest, CategoryDispatcher, DownloadEngine, DownloadStatus, RemoteZipPreviewer,
    SegmentStatus, TokenBucketLimiter,
};
use flate2::write::DeflateEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

/// Spawns a lightweight embedded HTTP/1.1 mock server supporting HEAD, GET, and RFC 9110 Range requests.
async fn spawn_mock_range_server(payload: Arc<Vec<u8>>, filename: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock TCP listener");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let data = Arc::clone(&payload);
            tokio::spawn(async move {
                let mut buf = Vec::with_capacity(2048);
                let mut tmp = [0u8; 1024];

                loop {
                    match socket.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                            if buf.len() > 16384 {
                                break;
                            }
                        }
                        Err(_) => return,
                    }
                }

                let req_str = String::from_utf8_lossy(&buf);
                let first_line = req_str.lines().next().unwrap_or("");
                let method = first_line.split_whitespace().next().unwrap_or("GET");
                let total_len = data.len();

                let mut range_header: Option<String> = None;
                for line in req_str.lines().skip(1) {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("range") {
                            range_header = Some(v.trim().to_string());
                        }
                    }
                }

                if method.eq_ignore_ascii_case("HEAD") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Disposition: attachment; filename=\"{}\"\r\nConnection: close\r\n\r\n",
                        total_len, filename
                    );
                    let _ = socket.write_all(hdr.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }

                if let Some(rh) = range_header {
                    if let Some(spec) = rh.strip_prefix("bytes=") {
                        if let Some((start_s, end_s)) = spec.split_once('-') {
                            let start = start_s.parse::<usize>().unwrap_or(0);
                            let end = if end_s.is_empty() {
                                total_len.saturating_sub(1)
                            } else {
                                end_s
                                    .parse::<usize>()
                                    .unwrap_or_else(|_| total_len.saturating_sub(1))
                                    .min(total_len.saturating_sub(1))
                            };

                            if start <= end && start < total_len {
                                let slice = &data[start..=end];
                                let hdr = format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                    slice.len(),
                                    start,
                                    end,
                                    total_len
                                );
                                let _ = socket.write_all(hdr.as_bytes()).await;
                                let _ = socket.write_all(slice).await;
                                let _ = socket.shutdown().await;
                                return;
                            }
                        }
                    }
                }

                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    total_len
                );
                let _ = socket.write_all(hdr.as_bytes()).await;
                let _ = socket.write_all(&data).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    format!("http://127.0.0.1:{}/{}", addr.port(), filename)
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds a valid PKZIP archive in memory for RemoteZipPreviewer tests (supports Stored=0 & Deflated=8).
fn build_test_zip(entries: &[(&str, &[u8], u16)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd_records = Vec::new();

    for &(name, raw_data, method) in entries {
        let crc = crc32_ieee(raw_data);
        let compressed: Vec<u8> = match method {
            8 => {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                enc.write_all(raw_data).unwrap();
                enc.finish().unwrap()
            }
            _ => raw_data.to_vec(),
        };

        let local_offset = out.len() as u32;
        let name_bytes = name.as_bytes();

        // Local File Header (30 bytes + filename)
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(raw_data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&compressed);

        // Central Directory Header (46 bytes + filename)
        cd_records.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        cd_records.extend_from_slice(&20u16.to_le_bytes()); // version made by
        cd_records.extend_from_slice(&20u16.to_le_bytes()); // version needed
        cd_records.extend_from_slice(&0u16.to_le_bytes()); // flags
        cd_records.extend_from_slice(&method.to_le_bytes());
        cd_records.extend_from_slice(&0u16.to_le_bytes());
        cd_records.extend_from_slice(&0u16.to_le_bytes());
        cd_records.extend_from_slice(&crc.to_le_bytes());
        cd_records.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        cd_records.extend_from_slice(&(raw_data.len() as u32).to_le_bytes());
        cd_records.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        cd_records.extend_from_slice(&0u16.to_le_bytes()); // extra len
        cd_records.extend_from_slice(&0u16.to_le_bytes()); // comment len
        cd_records.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        cd_records.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        cd_records.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        cd_records.extend_from_slice(&local_offset.to_le_bytes());
        cd_records.extend_from_slice(name_bytes);
    }

    let cd_offset = out.len() as u32;
    let cd_size = cd_records.len() as u32;
    out.extend_from_slice(&cd_records);

    // End of Central Directory (22 bytes)
    let count = entries.len() as u16;
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk num
    out.extend_from_slice(&0u16.to_le_bytes()); // start disk
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len

    out
}

#[tokio::test]
async fn test_parallel_range_download_and_dynamic_reassembly() {
    // >= 512 KB triggers multi-connection segmentation in DownloadEngine
    let size = 768 * 1024; // 768 KB
    let payload: Vec<u8> = (0..size)
        .map(|i| ((i * 31 + (i >> 8)) & 0xFF) as u8)
        .collect();
    let expected_sha256 = hex::encode(Sha256::digest(&payload));

    let url = spawn_mock_range_server(Arc::new(payload), "test_bundle.bin").await;

    let temp_root = std::env::temp_dir().join(format!("octoheft_it_dl_{}", Uuid::new_v4()));
    let storage_dir = temp_root.join("state");
    let download_dir = temp_root.join("downloads");
    std::fs::create_dir_all(&storage_dir).unwrap();
    std::fs::create_dir_all(&download_dir).unwrap();

    let dispatcher = Arc::new(CategoryDispatcher::new(&storage_dir, download_dir.clone()));
    let engine = DownloadEngine::new(&storage_dir, download_dir.clone(), dispatcher);

    let task = engine
        .add_download(
            AddDownloadRequest {
                url,
                save_path: Some(download_dir.to_string_lossy().to_string()),
                file_name: Some("test_bundle.bin".to_string()),
                connections: Some(4),
                headers: None,
                use_category_subfolders: None,
            },
            false,
            None,
        )
        .await
        .expect("add_download failed");

    assert!(task.supports_range, "Server should report range support");
    assert_eq!(task.segments.len(), 4, "Should partition into 4 parallel segments");
    assert_eq!(task.total_size, size as u64);

    engine
        .start_download_headless(&task.id)
        .expect("start_download_headless failed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let final_task = loop {
        let t = engine.state_store.get_task(&task.id).unwrap();
        if t.status == DownloadStatus::Completed || t.status == DownloadStatus::Error {
            break t;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for parallel download completion"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(
        final_task.status, DownloadStatus::Completed,
        "Expected Completed, got error: {:?}",
        final_task.error_message
    );
    assert_eq!(final_task.downloaded_bytes, size as u64);
    assert!(
        final_task
            .segments
            .iter()
            .all(|s| s.status == SegmentStatus::Completed),
        "All 4 segments must complete cleanly"
    );

    let disk_bytes = std::fs::read(&final_task.save_path).expect("Output file missing on disk");
    assert_eq!(disk_bytes.len(), size);
    let actual_sha256 = hex::encode(Sha256::digest(&disk_bytes));
    assert_eq!(
        actual_sha256, expected_sha256,
        "Parallel reassembled file SHA-256 mismatch!"
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_token_bucket_rate_limiter_bandwidth_accuracy() {
    // 40,000 bytes/sec cap
    let limiter = TokenBucketLimiter::new(40_000);

    // Exhaust initial 1-second burst capacity
    limiter.acquire(40_000).await;

    // Next 30,000 bytes at 40,000 B/s must take ~0.75s
    let start = Instant::now();
    for _ in 0..3 {
        limiter.acquire(10_000).await;
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(650),
        "Token bucket failed to throttle accurately; 30KB took only {:?}",
        elapsed
    );
    assert!(
        elapsed <= Duration::from_secs(3),
        "Token bucket over-throttled excessively: {:?}",
        elapsed
    );

    // Setting rate to 0 disables throttling immediately
    limiter.set_rate(0);
    let unlimited_start = Instant::now();
    limiter.acquire(512_000).await;
    assert!(
        unlimited_start.elapsed() < Duration::from_millis(100),
        "Unlimited rate (0) should return immediately"
    );
}

#[tokio::test]
async fn test_sparse_file_preallocation_and_concurrent_seek_writes() {
    let temp_path: PathBuf = std::env::temp_dir().join(format!("octoheft_it_sparse_{}.bin", Uuid::new_v4()));
    let total_size = 1_048_576u64; // 1 MB

    let sparse = Arc::new(
        SparseFile::create_preallocated(&temp_path, total_size).expect("Preallocation failed"),
    );

    let writes: Vec<(u64, &'static [u8])> = vec![
        (0, b"OCTOHEFT_HDR_00"),
        (262_144, b"OCTOHEFT_QTR_01"),
        (524_288, b"OCTOHEFT_MID_02"),
        (999_424, b"OCTOHEFT_END_03"),
    ];

    let mut handles = Vec::new();
    for (offset, payload) in writes.clone() {
        let s = Arc::clone(&sparse);
        handles.push(tokio::spawn(async move {
            s.write_at(offset, payload).expect("seek_write failed");
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
    sparse.sync_all().unwrap();

    let mut f = std::fs::File::open(&temp_path).unwrap();
    assert_eq!(f.metadata().unwrap().len(), total_size);

    for (offset, expected) in writes {
        let mut buf = vec![0u8; expected.len()];
        f.seek(SeekFrom::Start(offset)).unwrap();
        f.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], expected, "Mismatch at offset {}", offset);
    }

    // Verify untouched intermediate region stays zero-filled
    let mut zero_probe = [0xFFu8; 64];
    f.seek(SeekFrom::Start(128_000)).unwrap();
    f.read_exact(&mut zero_probe).unwrap();
    assert!(zero_probe.iter().all(|&b| b == 0));

    drop(f);
    drop(sparse);
    let _ = std::fs::remove_file(&temp_path);
}

#[tokio::test]
async fn test_remote_zip_trailing_parser_and_selective_extraction() {
    let text_stored = b"Hello from Stored ZIP entry in OctoHeft integration test!";
    let text_deflated =
        b"Compressed DEFLATE entry payload for selective byte-range ZIP extraction test. "
            .repeat(8);

    let zip_bytes = build_test_zip(&[
        ("docs/hello.txt", text_stored, 0),
        ("data/deflated.txt", &text_deflated, 8),
    ]);

    let url = spawn_mock_range_server(Arc::new(zip_bytes.clone()), "archive_test.zip").await;
    let client = reqwest::Client::new();

    // 1. Inspect EOCD & Central Directory using trailing HTTP Range request
    let preview = RemoteZipPreviewer::preview_remote_zip(&client, &url)
        .await
        .expect("Remote ZIP preview failed");

    assert_eq!(preview.total_archive_size, zip_bytes.len() as u64);
    assert_eq!(preview.entries_count, 2);
    assert_eq!(preview.files[0].name, "docs/hello.txt");
    assert_eq!(preview.files[0].compression_name, "Stored");
    assert_eq!(preview.files[0].uncompressed_size, text_stored.len() as u64);
    assert_eq!(preview.files[1].name, "data/deflated.txt");
    assert_eq!(preview.files[1].compression_name, "Deflated");
    assert_eq!(
        preview.files[1].uncompressed_size,
        text_deflated.len() as u64
    );

    // 2. Selectively extract only the deflated entry via byte-range slicing
    let extract_dir = std::env::temp_dir().join(format!("octoheft_it_zip_{}", Uuid::new_v4()));
    let extract_path = extract_dir.join("deflated.txt");

    RemoteZipPreviewer::extract_single_entry(&client, &url, &preview.files[1], &extract_path)
        .await
        .expect("Selective ZIP extraction failed");

    let extracted = std::fs::read(&extract_path).unwrap();
    assert_eq!(extracted, text_deflated);

    // 3. Corrupt stream / non-ZIP target should return Err cleanly without panicking
    let corrupt_url =
        spawn_mock_range_server(Arc::new(vec![0xAAu8; 256]), "corrupt.zip").await;
    let err_res = RemoteZipPreviewer::preview_remote_zip(&client, &corrupt_url).await;
    assert!(err_res.is_err(), "Corrupt archive should fail explicitly");

    let _ = std::fs::remove_dir_all(&extract_dir);
}

#[tokio::test]
async fn test_graceful_error_recovery_on_premature_eof() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Server advertises 64 KB on HEAD, but drops TCP connection after only 512 bytes on GET
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let req = String::from_utf8_lossy(&buf);
                let is_head = req.starts_with("HEAD ");
                if is_head {
                    let hdr = "HTTP/1.1 200 OK\r\nContent-Length: 65536\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(hdr.as_bytes()).await;
                } else {
                    let hdr = "HTTP/1.1 206 Partial Content\r\nContent-Length: 65536\r\nContent-Range: bytes 0-65535/65536\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(hdr.as_bytes()).await;
                    let _ = socket.write_all(&[0x5Au8; 512]).await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });

    let url = format!("http://127.0.0.1:{}/truncated.bin", addr.port());
    let temp_root = std::env::temp_dir().join(format!("octoheft_it_err_{}", Uuid::new_v4()));
    let storage_dir = temp_root.join("state");
    let download_dir = temp_root.join("downloads");
    std::fs::create_dir_all(&storage_dir).unwrap();
    std::fs::create_dir_all(&download_dir).unwrap();

    let dispatcher = Arc::new(CategoryDispatcher::new(&storage_dir, download_dir.clone()));
    let engine = DownloadEngine::new(&storage_dir, download_dir.clone(), dispatcher);

    let task = engine
        .add_download(
            AddDownloadRequest {
                url,
                save_path: Some(download_dir.to_string_lossy().to_string()),
                file_name: Some("truncated.bin".to_string()),
                connections: Some(1),
                headers: None,
                use_category_subfolders: None,
            },
            false,
            None,
        )
        .await
        .expect("add_download failed");

    engine
        .start_download_headless(&task.id)
        .expect("start_download_headless failed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let final_task = loop {
        let t = engine.state_store.get_task(&task.id).unwrap();
        if t.status == DownloadStatus::Error || t.status == DownloadStatus::Completed {
            break t;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for EngineError recovery state"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(final_task.status, DownloadStatus::Error);
    let err_msg = final_task.error_message.unwrap_or_default();
    assert!(
        err_msg.contains("Corrupted or invalid byte stream") || err_msg.contains("Premature EOF"),
        "Unexpected error message: {}",
        err_msg
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_download_engine_enforces_session_headers_and_handles_403() {
    use std::collections::HashMap;

    let size = 600 * 1024;
    let payload: Vec<u8> = (0..size).map(|i| ((i * 17 + 3) & 0xFF) as u8).collect();
    let expected_sha256 = hex::encode(Sha256::digest(&payload));
    let payload_arc = Arc::new(payload);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let p_clone = Arc::clone(&payload_arc);

    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let data = Arc::clone(&p_clone);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let req_lc = req.to_ascii_lowercase();
                let ok = req_lc.contains("user-agent: mozilla/5.0 session-test")
                    && req_lc.contains("referer: https://www.youtube.com/")
                    && req_lc.contains("origin: https://www.youtube.com")
                    && req_lc.contains("cookie: sid=ok_token");

                if !ok {
                    let hdr = "HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nFORBIDDEN";
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.shutdown().await;
                    return;
                }

                let method = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().next())
                    .unwrap_or("GET");
                let total = data.len();

                if method.eq_ignore_ascii_case("HEAD") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        total
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.shutdown().await;
                    return;
                }

                if let Some(rl) = req_lc.lines().find(|l| l.starts_with("range:")) {
                    let spec = rl.split(':').nth(1).unwrap_or("").trim().trim_start_matches("bytes=");
                    let mut parts = spec.split('-');
                    let s = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
                    let e = parts
                        .next()
                        .and_then(|x| x.parse::<usize>().ok())
                        .unwrap_or(total - 1)
                        .min(total - 1);
                    let slice = &data[s..=e];
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        s, e, total, slice.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                } else {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        total
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(&data).await;
                }
                let _ = sock.shutdown().await;
            });
        }
    });

    let url = format!("http://127.0.0.1:{}/videoplayback?itag=137", port);
    let temp_root = std::env::temp_dir().join(format!("octoheft_it_hdrs_{}", Uuid::new_v4()));
    let storage_dir = temp_root.join("state");
    let download_dir = temp_root.join("downloads");
    std::fs::create_dir_all(&storage_dir).unwrap();
    std::fs::create_dir_all(&download_dir).unwrap();

    let dispatcher = Arc::new(CategoryDispatcher::new(&storage_dir, download_dir.clone()));
    let engine = DownloadEngine::new(&storage_dir, download_dir.clone(), dispatcher);

    // 1. Missing headers must fail probe_url explicitly with HTTP 403
    let err_missing = engine
        .add_download(
            AddDownloadRequest {
                url: url.clone(),
                save_path: Some(download_dir.to_string_lossy().to_string()),
                file_name: Some("blocked.mp4".to_string()),
                connections: Some(4),
                headers: None,
                use_category_subfolders: None,
            },
            false,
            None,
        )
        .await;
    assert!(err_missing.is_err(), "Missing session headers must fail 403");
    assert!(err_missing.unwrap_err().contains("403"));

    // 2. Full browser session headers must succeed across parallel chunks
    let mut hdrs = HashMap::new();
    hdrs.insert("User-Agent".to_string(), "Mozilla/5.0 session-test".to_string());
    hdrs.insert("Referer".to_string(), "https://www.youtube.com/".to_string());
    hdrs.insert("Origin".to_string(), "https://www.youtube.com".to_string());
    hdrs.insert("Cookie".to_string(), "SID=ok_token".to_string());

    let task = engine
        .add_download(
            AddDownloadRequest {
                url,
                save_path: Some(download_dir.to_string_lossy().to_string()),
                file_name: Some("authorized.mp4".to_string()),
                connections: Some(4),
                headers: Some(hdrs),
                use_category_subfolders: None,
            },
            false,
            None,
        )
        .await
        .expect("Authorized download should add cleanly");

    engine.start_download_headless(&task.id).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let final_task = loop {
        let t = engine.state_store.get_task(&task.id).unwrap();
        if t.status == DownloadStatus::Completed || t.status == DownloadStatus::Error {
            break t;
        }
        assert!(Instant::now() < deadline, "Timed out waiting for header-enforced download");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(final_task.status, DownloadStatus::Completed, "Error: {:?}", final_task.error_message);
    let disk_bytes = std::fs::read(&final_task.save_path).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&disk_bytes)), expected_sha256);

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_telemetry_ewma_stability_and_zero_speed_hysteresis() {
    // 8 MB payload delivered in 2 bursts separated by a ~1.2s pause:
    // - Tick 1 (500ms): records initial burst speed (> 0)
    // - Tick 2 (1000ms): observes 0-byte pause window (1st zero tick < 1s grace window) -> speed MUST stay > 0
    // - Resume (1200ms): finishes second burst cleanly and resets speed to 0 on completion.
    let size = 8 * 1024 * 1024;
    let payload: Arc<Vec<u8>> = Arc::new((0..size).map(|i| ((i * 13 + 7) & 0xFF) as u8).collect());
    let expected_sha256 = hex::encode(Sha256::digest(&*payload));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let payload_srv = Arc::clone(&payload);

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let data = Arc::clone(&payload_srv);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let req_lc = req.to_ascii_lowercase();
                let method = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().next())
                    .unwrap_or("GET");
                let total = data.len();

                if method.eq_ignore_ascii_case("HEAD") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        total
                    );
                    let _ = socket.write_all(hdr.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }

                let (s, e) = if let Some(rl) = req_lc.lines().find(|l| l.starts_with("range:")) {
                    let spec = rl.split(':').nth(1).unwrap_or("").trim().trim_start_matches("bytes=");
                    let mut parts = spec.split('-');
                    let s = parts.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
                    let e = parts
                        .next()
                        .and_then(|x| x.parse::<usize>().ok())
                        .unwrap_or(total - 1)
                        .min(total - 1);
                    (s, e)
                } else {
                    (0, total - 1)
                };

                let slice = &data[s..=e];
                let mid = slice.len() / 2;
                let hdr = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    s,
                    e,
                    total,
                    slice.len()
                );
                let _ = socket.write_all(hdr.as_bytes()).await;
                let _ = socket.write_all(&slice[..mid]).await;
                let _ = socket.flush().await;

                // Simulate a transient TCP stall spanning the second 500ms telemetry tick
                tokio::time::sleep(Duration::from_millis(1200)).await;

                let _ = socket.write_all(&slice[mid..]).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    let url = format!("http://127.0.0.1:{}/burst_test.bin", addr.port());
    let temp_root = std::env::temp_dir().join(format!("octoheft_it_ewma_{}", Uuid::new_v4()));
    let storage_dir = temp_root.join("state");
    let download_dir = temp_root.join("downloads");
    std::fs::create_dir_all(&storage_dir).unwrap();
    std::fs::create_dir_all(&download_dir).unwrap();

    let dispatcher = Arc::new(CategoryDispatcher::new(&storage_dir, download_dir.clone()));
    let engine = DownloadEngine::new(&storage_dir, download_dir.clone(), dispatcher);

    let task = engine
        .add_download(
            AddDownloadRequest {
                url,
                save_path: Some(download_dir.to_string_lossy().to_string()),
                file_name: Some("burst_test.bin".to_string()),
                connections: Some(4),
                headers: None,
                use_category_subfolders: None,
            },
            false,
            None,
        )
        .await
        .expect("add_download failed");

    engine.start_download_headless(&task.id).unwrap();

    let started = Instant::now();
    let mut saw_positive_speed_at_tick1 = false;
    let mut dropped_to_zero_during_grace = false;
    let deadline = started + Duration::from_secs(10);

    let final_task = loop {
        let t = engine.state_store.get_task(&task.id).unwrap();
        let elapsed = started.elapsed();

        if elapsed >= Duration::from_millis(600) && elapsed <= Duration::from_millis(1150) {
            if t.speed > 0 {
                saw_positive_speed_at_tick1 = true;
            } else if saw_positive_speed_at_tick1 && t.status == DownloadStatus::Downloading {
                dropped_to_zero_during_grace = true;
            }
        }

        if t.status == DownloadStatus::Completed || t.status == DownloadStatus::Error {
            break t;
        }
        assert!(Instant::now() < deadline, "Timed out waiting for burst transfer completion");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(final_task.status, DownloadStatus::Completed, "Error: {:?}", final_task.error_message);
    assert_eq!(final_task.speed, 0, "Completed task must reset speed to 0");
    assert!(saw_positive_speed_at_tick1, "Expected smoothed non-zero speed after 500ms tick");
    assert!(
        !dropped_to_zero_during_grace,
        "Displayed speed must NOT drop to 0 during a single 500ms stall tick (< 1s grace window)"
    );

    let disk_bytes = std::fs::read(&final_task.save_path).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&disk_bytes)), expected_sha256);

    let _ = std::fs::remove_dir_all(&temp_root);
}


