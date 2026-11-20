use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tracing::{debug, error, info, warn};

pub const GITHUB_RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/Th3Gold3nRainbow/OctoHeft/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: Option<String>,
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgressPayload {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: f64,
    pub status: String, // "downloading" | "ready" | "error"
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    assets: Vec<GithubReleaseAsset>,
}

struct UpdateCache {
    last_checked: Instant,
    cached_info: UpdateInfo,
}

static UPDATE_CACHE: parking_lot::Mutex<Option<UpdateCache>> = parking_lot::Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(15 * 60); // 15 minutes cache

/// Parses a semver string like "v0.1.2", "0.1.2", "v0.1.2-alpha" into (major, minor, patch)
pub fn parse_version_tuple(v: &str) -> Option<(u64, u64, u64)> {
    let clean = v.trim().trim_start_matches(|c| c == 'v' || c == 'V');
    let mut parts = clean.split('.');
    let major = parts.next()?.split('-').next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").split('-').next()?.parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").split('-').next()?.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// Returns true if `latest` is strictly newer than `current`
pub fn is_version_newer(latest: &str, current: &str) -> bool {
    let l_clean = latest.trim().trim_start_matches(|ch| ch == 'v' || ch == 'V');
    let c_clean = current.trim().trim_start_matches(|ch| ch == 'v' || ch == 'V');

    if let (Ok(l), Ok(c)) = (semver::Version::parse(l_clean), semver::Version::parse(c_clean)) {
        return l > c;
    }

    match (parse_version_tuple(latest), parse_version_tuple(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Checks the GitHub Releases API for new releases.
/// Handles network and parsing errors silently without throwing intrusive errors.
pub async fn check_update(force: bool) -> Result<UpdateInfo, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    if !force {
        let lock = UPDATE_CACHE.lock();
        if let Some(ref cache) = *lock {
            if cache.last_checked.elapsed() < CACHE_TTL {
                return Ok(cache.cached_info.clone());
            }
        }
    }

    // Optional environment simulation for testing/QA
    if let Ok(sim_tag) = std::env::var("OCTOHEFT_SIMULATE_UPDATE") {
        let trimmed = sim_tag.trim();
        if !trimmed.is_empty() {
            let newer = is_version_newer(trimmed, &current_version);
            let sim_info = UpdateInfo {
                available: newer,
                current_version: current_version.clone(),
                latest_version: trimmed.trim_start_matches(|c| c == 'v' || c == 'V').to_string(),
                release_notes: Some("Simulated release notes for in-app updater testing.".to_string()),
                download_url: Some("https://github.com/Th3Gold3nRainbow/OctoHeft/releases/latest".to_string()),
            };
            return Ok(sim_info);
        }
    }

    let client = match reqwest::Client::builder()
        .user_agent(format!("OctoHeft-App/{}", current_version))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to build reqwest client for update check: {}", e);
            return Ok(UpdateInfo {
                available: false,
                current_version: current_version.clone(),
                latest_version: current_version,
                release_notes: None,
                download_url: None,
            });
        }
    };

    let resp = match client.get(GITHUB_RELEASES_LATEST_URL).send().await {
        Ok(r) => r,
        Err(e) => {
            debug!("Update check network unreachable: {}", e);
            return Ok(UpdateInfo {
                available: false,
                current_version: current_version.clone(),
                latest_version: current_version,
                release_notes: None,
                download_url: None,
            });
        }
    };

    if !resp.status().is_success() {
        debug!("Update check returned HTTP status: {}", resp.status());
        return Ok(UpdateInfo {
            available: false,
            current_version: current_version.clone(),
            latest_version: current_version,
            release_notes: None,
            download_url: None,
        });
    }

    let release = match resp.json::<GithubRelease>().await {
        Ok(rel) => rel,
        Err(e) => {
            debug!("Failed to deserialize GitHub release JSON: {}", e);
            return Ok(UpdateInfo {
                available: false,
                current_version: current_version.clone(),
                latest_version: current_version,
                release_notes: None,
                download_url: None,
            });
        }
    };

    let available = is_version_newer(&release.tag_name, &current_version);
    let latest_clean = release
        .tag_name
        .trim_start_matches(|c| c == 'v' || c == 'V')
        .to_string();

    let download_url = release
        .assets
        .iter()
        .find(|a| {
            let lower = a.name.to_ascii_lowercase();
            lower.ends_with(".exe")
                && (lower.contains("setup") || lower.contains("installer") || lower.contains("x64"))
        })
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|a| a.name.to_ascii_lowercase().ends_with(".exe"))
        })
        .map(|a| a.browser_download_url.clone());

    let info = UpdateInfo {
        available,
        current_version,
        latest_version: latest_clean,
        release_notes: release.body,
        download_url,
    };

    let mut lock = UPDATE_CACHE.lock();
    *lock = Some(UpdateCache {
        last_checked: Instant::now(),
        cached_info: info.clone(),
    });

    Ok(info)
}

/// Streams the update installer binary to `%TEMP%\OctoHeft_Update.exe`,
/// emits progress events (`update://progress`), launches the installer,
/// and terminates the application cleanly.
pub async fn install_update(download_url: String, app: AppHandle) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    info!(url = %download_url, "Starting in-app update download");

    let client = reqwest::Client::builder()
        .user_agent(format!("OctoHeft-App/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Network request failed: {}", e))?;

    if !response.status().is_success() {
        let err_msg = format!("Download failed with HTTP status: {}", response.status());
        let _ = app.emit(
            "update://progress",
            &UpdateProgressPayload {
                downloaded_bytes: 0,
                total_bytes: None,
                percent: 0.0,
                status: "error".to_string(),
                error_message: Some(err_msg.clone()),
            },
        );
        return Err(err_msg);
    }

    let total_bytes = response.content_length();
    let temp_file_path = std::env::temp_dir().join("OctoHeft_Update.exe");

    let mut file = tokio::fs::File::create(&temp_file_path)
        .await
        .map_err(|e| format!("Failed to create temporary update installer: {}", e))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now();

    while let Some(chunk_res) = stream.next().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("Streaming error: {}", e);
                let _ = app.emit(
                    "update://progress",
                    &UpdateProgressPayload {
                        downloaded_bytes: downloaded,
                        total_bytes,
                        percent: 0.0,
                        status: "error".to_string(),
                        error_message: Some(err_msg.clone()),
                    },
                );
                return Err(err_msg);
            }
        };

        if let Err(e) = file.write_all(&chunk).await {
            let err_msg = format!("Disk write error: {}", e);
            let _ = app.emit(
                "update://progress",
                &UpdateProgressPayload {
                    downloaded_bytes: downloaded,
                    total_bytes,
                    percent: 0.0,
                    status: "error".to_string(),
                    error_message: Some(err_msg.clone()),
                },
            );
            return Err(err_msg);
        }

        downloaded += chunk.len() as u64;

        if last_emit.elapsed() >= Duration::from_millis(100) {
            let percent = total_bytes
                .map(|tot| if tot > 0 { (downloaded as f64 / tot as f64) * 100.0 } else { 0.0 })
                .unwrap_or(0.0);

            let _ = app.emit(
                "update://progress",
                &UpdateProgressPayload {
                    downloaded_bytes: downloaded,
                    total_bytes,
                    percent,
                    status: "downloading".to_string(),
                    error_message: None,
                },
            );
            last_emit = Instant::now();
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush installer: {}", e))?;
    drop(file);

    info!(bytes = downloaded, "Update download completed successfully");

    // Emit ready status
    let _ = app.emit(
        "update://progress",
        &UpdateProgressPayload {
            downloaded_bytes: downloaded,
            total_bytes,
            percent: 100.0,
            status: "ready".to_string(),
            error_message: None,
        },
    );

    // Allow frontend to render ready confirmation
    tokio::time::sleep(Duration::from_millis(1500)).await;

    #[cfg(target_os = "windows")]
    {
        match std::process::Command::new(&temp_file_path).spawn() {
            Ok(_) => {
                info!("Installer spawned successfully, terminating application for clean update");
                tokio::time::sleep(Duration::from_millis(400)).await;
                app.exit(0);
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Failed to launch update installer: {}", e);
                error!("{}", err_msg);
                let _ = app.emit(
                    "update://progress",
                    &UpdateProgressPayload {
                        downloaded_bytes: downloaded,
                        total_bytes,
                        percent: 100.0,
                        status: "error".to_string(),
                        error_message: Some(err_msg.clone()),
                    },
                );
                Err(err_msg)
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(())
    }
}

/// Spawns the downloaded installer manually if needed
pub fn launch_update(app: AppHandle) -> Result<(), String> {
    let temp_file_path = std::env::temp_dir().join("OctoHeft_Update.exe");
    if !temp_file_path.exists() {
        return Err("Update installer binary not found".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(&temp_file_path)
            .spawn()
            .map_err(|e| format!("Failed to launch installer: {}", e))?;
        app.exit(0);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparisons() {
        assert!(is_version_newer("v0.1.2", "0.1.1"));
        assert!(is_version_newer("0.1.2", "0.1.1"));
        assert!(is_version_newer("v0.2.0", "0.1.1"));
        assert!(is_version_newer("v1.0.0", "0.1.1"));
        assert!(!is_version_newer("v0.1.1", "0.1.1"));
        assert!(!is_version_newer("v0.1.0", "0.1.1"));
        assert!(!is_version_newer("0.1.0", "0.1.1"));
        assert!(is_version_newer("v0.1.2-alpha", "0.1.1"));
    }

    #[test]
    fn test_parse_version_tuple() {
        assert_eq!(parse_version_tuple("v0.1.2"), Some((0, 1, 2)));
        assert_eq!(parse_version_tuple("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version_tuple("v2.0.0-beta"), Some((2, 0, 0)));
    }
}
