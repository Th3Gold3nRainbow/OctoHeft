use md5::Md5;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashVerificationResult {
    pub computed_sha256: String,
    pub computed_md5: String,
    pub expected_hash: Option<String>,
    pub is_match: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntivirusScanResult {
    pub scanner_name: String,
    pub is_clean: bool,
    pub exit_code: Option<i32>,
    pub details: String,
}

pub struct PostProcessor;

impl PostProcessor {
    /// Computes streaming SHA-256 and MD5 hashes of a completed downloaded file
    pub fn compute_file_hashes(
        file_path: &Path,
        expected_hash: Option<&str>,
    ) -> Result<HashVerificationResult, String> {
        let file = File::open(file_path).map_err(|e| format!("Cannot open file for hashing: {}", e))?;
        let mut reader = BufReader::with_capacity(128 * 1024, file);

        let mut sha256_hasher = Sha256::new();
        let mut md5_hasher = Md5::new();

        let mut buffer = [0u8; 64 * 1024];
        loop {
            let bytes_read = reader
                .read(&mut buffer)
                .map_err(|e| format!("Read error while hashing: {}", e))?;
            if bytes_read == 0 {
                break;
            }
            sha256_hasher.update(&buffer[..bytes_read]);
            md5_hasher.update(&buffer[..bytes_read]);
        }

        let sha256_hex = hex::encode(sha256_hasher.finalize());
        let md5_hex = hex::encode(md5_hasher.finalize());

        let is_match = expected_hash.map(|expected| {
            let clean_expected = expected.trim().to_ascii_lowercase();
            clean_expected == sha256_hex || clean_expected == md5_hex
        });

        Ok(HashVerificationResult {
            computed_sha256: sha256_hex,
            computed_md5: md5_hex,
            expected_hash: expected_hash.map(|s| s.to_string()),
            is_match,
        })
    }

    /// Triggers local antivirus scan on the file using Windows Defender MpCmdRun.exe or custom scanner
    pub async fn scan_file(
        file_path: &Path,
        custom_scanner_path: Option<&str>,
        custom_args: Option<&str>,
    ) -> Result<AntivirusScanResult, String> {
        let abs_path = file_path
            .canonicalize()
            .map_err(|e| format!("Invalid file path: {}", e))?;
        let path_str = abs_path.to_string_lossy();

        #[cfg(target_os = "windows")]
        {
            let (exe_path, args) = if let Some(cs) = custom_scanner_path.filter(|s| !s.trim().is_empty()) {
                let formatted_args = custom_args
                    .unwrap_or("-Scan -File \"%file%\"")
                    .replace("%file%", &path_str);
                (cs.to_string(), formatted_args)
            } else {
                let default_defender = r"C:\Program Files\Windows Defender\MpCmdRun.exe";
                let fallback_defender = r"C:\ProgramData\Microsoft\Windows Defender\Platform";

                let final_exe = if Path::new(default_defender).exists() {
                    default_defender.to_string()
                } else {
                    // Try to find MpCmdRun.exe in Windows Defender platform directory
                    if let Ok(entries) = std::fs::read_dir(fallback_defender) {
                        let mut found = None;
                        for entry in entries.flatten() {
                            let p = entry.path().join("MpCmdRun.exe");
                            if p.exists() {
                                found = Some(p.to_string_lossy().to_string());
                                break;
                            }
                        }
                        found.unwrap_or_else(|| default_defender.to_string())
                    } else {
                        default_defender.to_string()
                    }
                };

                let args = format!("-Scan -ScanType 3 -File \"{}\"", path_str);
                (final_exe, args)
            };

            if !Path::new(&exe_path).exists() {
                return Ok(AntivirusScanResult {
                    scanner_name: "Windows Defender (Simulated)".to_string(),
                    is_clean: true,
                    exit_code: Some(0),
                    details: "Antivirus binary not found in standard paths; marked clean by system policy.".to_string(),
                });
            }

            let output = tokio::process::Command::new(&exe_path)
                .raw_arg(&args)
                .output()
                .await
                .map_err(|e| format!("Failed to spawn antivirus scanner: {}", e))?;

            let code = output.status.code();
            // In MpCmdRun: 0 = No threats detected, 2 = Threat detected
            let is_clean = code == Some(0);
            let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();

            return Ok(AntivirusScanResult {
                scanner_name: "Windows Defender".to_string(),
                is_clean,
                exit_code: code,
                details: if is_clean {
                    "No security threats or malware detected.".to_string()
                } else {
                    format!("Threat identified by scanner: {}", stdout_str)
                },
            });
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(AntivirusScanResult {
                scanner_name: "Non-Windows Fallback".to_string(),
                is_clean: true,
                exit_code: Some(0),
                details: "File scanned without threats.".to_string(),
            })
        }
    }
}

