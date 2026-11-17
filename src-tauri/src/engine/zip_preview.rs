use flate2::read::DeflateDecoder;
use reqwest::header::RANGE;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZipFileEntry {
    pub name: String,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compression_method: u16, // 0 = Stored, 8 = Deflated
    pub compression_name: String,
    pub crc32: u32,
    pub local_header_offset: u64,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZipPreviewResult {
    pub url: String,
    pub total_archive_size: u64,
    pub entries_count: usize,
    pub files: Vec<ZipFileEntry>,
}

pub struct RemoteZipPreviewer;

impl RemoteZipPreviewer {
    /// Inspects a remote ZIP archive by fetching trailing bytes to parse Central Directory without downloading the archive.
    pub async fn preview_remote_zip(
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ZipPreviewResult, String> {
        // 1. Get total Content-Length
        let head_res = client
            .head(url)
            .send()
            .await
            .map_err(|e| format!("HEAD request failed: {}", e))?;

        let total_size = head_res
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        if total_size < 22 {
            return Err("File is too small to be a valid ZIP archive".to_string());
        }

        // 2. Fetch trailing 65,536 bytes (or whole file if smaller)
        let fetch_len = std::cmp::min(total_size, 65536);
        let fetch_start = total_size - fetch_len;
        let range_hdr = format!("bytes={}-{}", fetch_start, total_size - 1);

        let tail_resp = client
            .get(url)
            .header(RANGE, range_hdr)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch ZIP trailing bytes: {}", e))?;

        if !tail_resp.status().is_success() && tail_resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("Server does not support range requests for ZIP preview: HTTP {}", tail_resp.status()));
        }

        let tail_bytes = tail_resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read trailing buffer: {}", e))?;

        // 3. Scan backward for End of Central Directory (EOCD) signature: 0x06054b50
        let eocd_sig = [0x50, 0x4b, 0x05, 0x06];
        let mut eocd_offset_in_buf = None;

        if tail_bytes.len() >= 22 {
            for i in (0..=tail_bytes.len() - 22).rev() {
                if tail_bytes[i..i + 4] == eocd_sig {
                    eocd_offset_in_buf = Some(i);
                    break;
                }
            }
        }

        let eocd_idx = eocd_offset_in_buf
            .ok_or_else(|| "Could not locate End of Central Directory record in ZIP".to_string())?;

        let eocd_slice = &tail_bytes[eocd_idx..];
        let total_entries = u16::from_le_bytes([eocd_slice[10], eocd_slice[11]]) as usize;
        let cd_size = u32::from_le_bytes([eocd_slice[12], eocd_slice[13], eocd_slice[14], eocd_slice[15]]) as u64;
        let cd_offset = u32::from_le_bytes([eocd_slice[16], eocd_slice[17], eocd_slice[18], eocd_slice[19]]) as u64;

        // 4. Obtain the Central Directory bytes
        let cd_bytes: Vec<u8> = if cd_offset >= fetch_start && (cd_offset + cd_size) <= total_size {
            // Already inside tail_bytes!
            let start = (cd_offset - fetch_start) as usize;
            let end = start + (cd_size as usize);
            if end <= tail_bytes.len() {
                tail_bytes[start..end].to_vec()
            } else {
                fetch_cd_range(client, url, cd_offset, cd_size).await?
            }
        } else {
            // Central Directory is larger or located before our 64KB window: fetch exact range
            fetch_cd_range(client, url, cd_offset, cd_size).await?
        };

        // 5. Parse Central Directory headers: 0x02014b50
        let cd_sig = [0x50, 0x4b, 0x01, 0x02];
        let mut files = Vec::with_capacity(total_entries);
        let mut cursor = 0;

        while cursor + 46 <= cd_bytes.len() {
            if cd_bytes[cursor..cursor + 4] != cd_sig {
                break;
            }

            let comp_method = u16::from_le_bytes([cd_bytes[cursor + 10], cd_bytes[cursor + 11]]);
            let crc32 = u32::from_le_bytes([
                cd_bytes[cursor + 16],
                cd_bytes[cursor + 17],
                cd_bytes[cursor + 18],
                cd_bytes[cursor + 19],
            ]);
            let comp_size = u32::from_le_bytes([
                cd_bytes[cursor + 20],
                cd_bytes[cursor + 21],
                cd_bytes[cursor + 22],
                cd_bytes[cursor + 23],
            ]) as u64;
            let uncomp_size = u32::from_le_bytes([
                cd_bytes[cursor + 24],
                cd_bytes[cursor + 25],
                cd_bytes[cursor + 26],
                cd_bytes[cursor + 27],
            ]) as u64;
            let fn_len = u16::from_le_bytes([cd_bytes[cursor + 28], cd_bytes[cursor + 29]]) as usize;
            let extra_len = u16::from_le_bytes([cd_bytes[cursor + 30], cd_bytes[cursor + 31]]) as usize;
            let comment_len = u16::from_le_bytes([cd_bytes[cursor + 32], cd_bytes[cursor + 33]]) as usize;
            let local_header_offset = u32::from_le_bytes([
                cd_bytes[cursor + 42],
                cd_bytes[cursor + 43],
                cd_bytes[cursor + 44],
                cd_bytes[cursor + 45],
            ]) as u64;

            cursor += 46;

            if cursor + fn_len > cd_bytes.len() {
                break;
            }

            let name_bytes = &cd_bytes[cursor..cursor + fn_len];
            let name = String::from_utf8_lossy(name_bytes).to_string();
            let is_directory = name.ends_with('/') || uncomp_size == 0 && comp_size == 0;

            let method_name = match comp_method {
                0 => "Stored",
                8 => "Deflated",
                14 => "LZMA",
                _ => "Other",
            }
            .to_string();

            files.push(ZipFileEntry {
                name,
                uncompressed_size: uncomp_size,
                compressed_size: comp_size,
                compression_method: comp_method,
                compression_name: method_name,
                crc32,
                local_header_offset,
                is_directory,
            });

            cursor += fn_len + extra_len + comment_len;
        }

        Ok(ZipPreviewResult {
            url: url.to_string(),
            total_archive_size: total_size,
            entries_count: files.len(),
            files,
        })
    }

    /// Selectively extracts a single file from within the remote ZIP archive using byte-range slicing
    pub async fn extract_single_entry(
        client: &reqwest::Client,
        url: &str,
        entry: &ZipFileEntry,
        save_path: &Path,
    ) -> Result<(), String> {
        if entry.is_directory {
            return Err("Cannot extract a directory entry as a file".to_string());
        }

        // 1. Fetch Local File Header (at least 30 bytes)
        let header_range = format!("bytes={}-{}", entry.local_header_offset, entry.local_header_offset + 30 + 1024);
        let resp = client
            .get(url)
            .header(RANGE, header_range)
            .send()
            .await
            .map_err(|e| format!("Failed to read local header: {}", e))?;

        let header_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read header body: {}", e))?;

        if header_bytes.len() < 30 || header_bytes[0..4] != [0x50, 0x4b, 0x03, 0x04] {
            return Err("Corrupt local file header signature in ZIP".to_string());
        }

        let fn_len = u16::from_le_bytes([header_bytes[26], header_bytes[27]]) as u64;
        let extra_len = u16::from_le_bytes([header_bytes[28], header_bytes[29]]) as u64;
        let data_start = entry.local_header_offset + 30 + fn_len + extra_len;
        let data_end = data_start + entry.compressed_size - 1;

        // 2. Fetch compressed payload bytes
        let data_range = format!("bytes={}-{}", data_start, data_end);
        let data_resp = client
            .get(url)
            .header(RANGE, data_range)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch entry payload: {}", e))?;

        let payload_bytes = data_resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read payload data: {}", e))?;

        // 3. Decompress if Deflated or save directly if Stored
        let final_data = match entry.compression_method {
            0 => payload_bytes.to_vec(),
            8 => {
                let mut decoder = DeflateDecoder::new(&payload_bytes[..]);
                let mut decompressed = Vec::with_capacity(entry.uncompressed_size as usize);
                decoder
                    .read_to_end(&mut decompressed)
                    .map_err(|e| format!("Decompression failed: {}", e))?;
                decompressed
            }
            other => return Err(format!("Unsupported compression method: {}", other)),
        };

        if let Some(parent) = save_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        std::fs::write(save_path, final_data)
            .map_err(|e| format!("Failed to save extracted file: {}", e))?;

        Ok(())
    }
}

async fn fetch_cd_range(client: &reqwest::Client, url: &str, offset: u64, size: u64) -> Result<Vec<u8>, String> {
    let range_hdr = format!("bytes={}-{}", offset, offset + size - 1);
    let resp = client
        .get(url)
        .header(RANGE, range_hdr)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch Central Directory: {}", e))?;

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read Central Directory data: {}", e))?;

    Ok(bytes.to_vec())
}

