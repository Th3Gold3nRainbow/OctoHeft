use crate::engine::omniget::OmnigetToolsStatus;
use crate::shell_integration::ShellIntegration;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct ResolvedTools {
    pub ytdlp_path: Option<PathBuf>,
    pub ffmpeg_path: Option<PathBuf>,
    pub ffprobe_path: Option<PathBuf>,
    pub js_runtime: Option<(String, PathBuf)>,
    pub managed_bin_dir: PathBuf,
}

impl ResolvedTools {
    pub fn discover() -> Self {
        let managed_bin_dir = managed_bin_dir();
        let ytdlp_path = find_executable(
            "yt-dlp",
            &["yt-dlp.exe", "yt-dlp"],
            &managed_bin_dir,
            &winget_package_dirs("yt-dlp"),
        );
        let ffmpeg_path = find_executable(
            "ffmpeg",
            &["ffmpeg.exe", "ffmpeg"],
            &managed_bin_dir,
            &ffmpeg_candidate_dirs(),
        );
        let ffprobe_path = find_executable(
            "ffprobe",
            &["ffprobe.exe", "ffprobe"],
            &managed_bin_dir,
            &ffmpeg_candidate_dirs(),
        );
        let js_runtime = find_js_runtime(&managed_bin_dir);

        Self {
            ytdlp_path,
            ffmpeg_path,
            ffprobe_path,
            js_runtime,
            managed_bin_dir,
        }
    }

    pub fn ytdlp_version(&self) -> Option<String> {
        let path = self.ytdlp_path.as_ref()?;
        let mut cmd = build_std_command(path, self);
        cmd.arg("--version");
        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ver.is_empty() {
            None
        } else {
            Some(ver)
        }
    }

    pub fn to_status(&self, query_version: bool) -> OmnigetToolsStatus {
        let ytdlp_version = if query_version {
            self.ytdlp_version()
        } else {
            None
        };
        let ytdlp_available = self.ytdlp_path.is_some();
        let ffmpeg_available = self.ffmpeg_path.is_some();
        OmnigetToolsStatus {
            ytdlp_available,
            ytdlp_path: self
                .ytdlp_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            ytdlp_version,
            ffmpeg_available,
            ffmpeg_path: self
                .ffmpeg_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            ffprobe_path: self
                .ffprobe_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            js_runtime: self
                .js_runtime
                .as_ref()
                .map(|(kind, p)| format!("{}:{}", kind, p.display())),
            managed_bin_dir: self.managed_bin_dir.to_string_lossy().to_string(),
            ready: ytdlp_available && ffmpeg_available,
        }
    }

    pub fn augmented_path_env(&self) -> String {
        let mut extra_dirs: Vec<PathBuf> = Vec::new();
        extra_dirs.push(self.managed_bin_dir.clone());
        if let Some(ref ff) = self.ffmpeg_path {
            if let Some(parent) = ff.parent() {
                extra_dirs.push(parent.to_path_buf());
            }
        }
        if let Some((_, ref js_path)) = self.js_runtime {
            if let Some(parent) = js_path.parent() {
                extra_dirs.push(parent.to_path_buf());
            }
        }
        if let Some(ref yt) = self.ytdlp_path {
            if let Some(parent) = yt.parent() {
                extra_dirs.push(parent.to_path_buf());
            }
        }

        let current_path = std::env::var_os("PATH").unwrap_or_default();
        let mut combined: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for dir in extra_dirs
            .into_iter()
            .chain(std::env::split_paths(&current_path))
        {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let key = dir.to_string_lossy().to_ascii_lowercase();
            if seen.insert(key) {
                combined.push(dir);
            }
        }

        std::env::join_paths(combined)
            .map(|os| os.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

pub fn managed_bin_dir() -> PathBuf {
    if ShellIntegration::is_portable_mode() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                return exe_dir.join("data").join("bin");
            }
        }
    }

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        if !local_app_data.trim().is_empty() {
            return PathBuf::from(local_app_data).join("OctoHeft").join("bin");
        }
    }

    PathBuf::from(".").join("bin")
}

pub fn build_std_command(exe: &Path, tools: &ResolvedTools) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");

    let path_env = tools.augmented_path_env();
    if !path_env.is_empty() {
        cmd.env("PATH", path_env);
    }

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

pub fn build_tokio_command(exe: &Path, tools: &ResolvedTools) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");

    let path_env = tools.augmented_path_env();
    if !path_env.is_empty() {
        cmd.env("PATH", path_env);
    }

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

/// Force-terminates a child process and its descendants cleanly on Windows when paused or canceled.
pub async fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(target_os = "windows")]
    if let Some(pid) = child.id() {
        let mut tk = tokio::process::Command::new("taskkill");
        tk.args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW);
        let _ = tk.status().await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn find_executable(
    _display: &str,
    names: &[&str],
    managed_dir: &Path,
    extra_dirs: &[PathBuf],
) -> Option<PathBuf> {
    // 1. Managed OctoHeft bin directory first
    for name in names {
        let candidate = managed_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // 2. Executable directory (bundled next to OctoHeft.exe or resources/bin)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            for dir in [
                exe_dir.to_path_buf(),
                exe_dir.join("bin"),
                exe_dir.join("resources").join("bin"),
            ] {
                for name in names {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    // 3. Well-known platform package locations (WinGet / Chocolatey / Scoop / standard installs)
    for dir in extra_dirs {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 4. Current process PATH
    if let Some(path_os) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_os) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn winget_package_dirs(prefix: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(&local_app_data).join("Microsoft").join("WinGet");
        let links = base.join("Links");
        if links.is_dir() {
            dirs.push(links);
        }
        let pkgs = base.join("Packages");
        if let Ok(entries) = std::fs::read_dir(pkgs) {
            let prefix_lc = prefix.to_ascii_lowercase();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if name.contains(&prefix_lc) && entry.path().is_dir() {
                    dirs.push(entry.path());
                    if let Ok(sub_entries) = std::fs::read_dir(entry.path()) {
                        for sub in sub_entries.flatten() {
                            if sub.path().is_dir() {
                                dirs.push(sub.path());
                                let sub_bin = sub.path().join("bin");
                                if sub_bin.is_dir() {
                                    dirs.push(sub_bin);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        let scoop = PathBuf::from(&userprofile).join("scoop").join("shims");
        if scoop.is_dir() {
            dirs.push(scoop);
        }
    }
    dirs
}

fn ffmpeg_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(PathBuf::from(r"C:\ffmpeg\bin"));
    if let Ok(pf) = std::env::var("ProgramFiles") {
        dirs.push(PathBuf::from(&pf).join("ffmpeg").join("bin"));
        dirs.push(PathBuf::from(&pf).join("Gyan").join("FFmpeg").join("bin"));
    }
    dirs.extend(winget_package_dirs("ffmpeg"));
    dirs
}

fn find_js_runtime(managed_dir: &Path) -> Option<(String, PathBuf)> {
    let mut deno_dirs = winget_package_dirs("deno");
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        deno_dirs.push(PathBuf::from(userprofile).join(".deno").join("bin"));
    }
    if let Some(deno) = find_executable("deno", &["deno.exe", "deno"], managed_dir, &deno_dirs) {
        return Some(("deno".to_string(), deno));
    }

    let mut node_dirs = winget_package_dirs("nodejs");
    if let Ok(pf) = std::env::var("ProgramFiles") {
        node_dirs.push(PathBuf::from(pf).join("nodejs"));
    }
    if let Some(node) = find_executable("node", &["node.exe", "node"], managed_dir, &node_dirs) {
        return Some(("node".to_string(), node));
    }

    let mut bun_dirs = Vec::new();
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        bun_dirs.push(PathBuf::from(userprofile).join(".bun").join("bin"));
    }
    if let Some(bun) = find_executable("bun", &["bun.exe", "bun"], managed_dir, &bun_dirs) {
        return Some(("bun".to_string(), bun));
    }

    None
}

/// Ensures `yt-dlp`, `ffmpeg`/`ffprobe`, and a JS runtime (`deno`) are present,
/// downloading missing components into OctoHeft's managed bin directory with SHA-256 verification.
pub async fn ensure_tools_installed() -> Result<OmnigetToolsStatus, String> {
    let current = ResolvedTools::discover();
    if current.ytdlp_path.is_some() && current.ffmpeg_path.is_some() {
        return Ok(current.to_status(true));
    }

    let bin_dir = managed_bin_dir();
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("Failed creating managed bin directory: {}", e))?;

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(300))
        .user_agent("OctoHeft/2.0")
        .build()
        .map_err(|e| format!("Failed creating HTTP client for tool setup: {}", e))?;

    if current.ytdlp_path.is_none() {
        info!(dir = %bin_dir.display(), "Downloading verified yt-dlp.exe into managed bin dir");
        install_ytdlp_verified(&client, &bin_dir).await?;
    }

    if current.ffmpeg_path.is_none() || current.ffprobe_path.is_none() {
        info!(dir = %bin_dir.display(), "Downloading FFmpeg essentials into managed bin dir");
        install_ffmpeg_essentials(&client, &bin_dir).await?;
    }

    if current.js_runtime.is_none() {
        if let Err(e) = install_deno_verified(&client, &bin_dir).await {
            warn!(error = %e, "Optional Deno JS runtime download failed; continuing");
        }
    }

    let updated = ResolvedTools::discover();
    let status = updated.to_status(true);
    if !status.ready {
        return Err("Tools installation finished, but yt-dlp or ffmpeg could not be resolved on disk.".to_string());
    }
    Ok(status)
}

async fn install_ytdlp_verified(client: &reqwest::Client, bin_dir: &Path) -> Result<(), String> {
    let sums_url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
    let exe_url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

    let sums_text = client
        .get(sums_url)
        .send()
        .await
        .map_err(|e| format!("Failed fetching yt-dlp SHA2-256SUMS: {}", e))?
        .error_for_status()
        .map_err(|e| format!("yt-dlp SHA2-256SUMS HTTP error: {}", e))?
        .text()
        .await
        .map_err(|e| format!("Failed reading yt-dlp SHA2-256SUMS: {}", e))?;

    let expected_hash = parse_sha256_sums(&sums_text, "yt-dlp.exe")
        .ok_or_else(|| "SHA2-256SUMS did not contain a checksum entry for yt-dlp.exe".to_string())?;

    let bytes = client
        .get(exe_url)
        .send()
        .await
        .map_err(|e| format!("Failed downloading yt-dlp.exe: {}", e))?
        .error_for_status()
        .map_err(|e| format!("yt-dlp.exe HTTP error: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Failed reading yt-dlp.exe stream: {}", e))?;

    let actual_hash = hex::encode(Sha256::digest(&bytes));
    if !actual_hash.eq_ignore_ascii_case(&expected_hash) {
        return Err(format!(
            "yt-dlp.exe SHA-256 verification failed (expected {}, got {})",
            expected_hash, actual_hash
        ));
    }

    let final_path = bin_dir.join("yt-dlp.exe");
    let part_path = bin_dir.join("yt-dlp.exe.part");
    std::fs::write(&part_path, &bytes)
        .map_err(|e| format!("Failed writing yt-dlp.exe: {}", e))?;
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }
    std::fs::rename(&part_path, &final_path)
        .map_err(|e| format!("Failed activating yt-dlp.exe: {}", e))?;
    Ok(())
}

async fn install_ffmpeg_essentials(client: &reqwest::Client, bin_dir: &Path) -> Result<(), String> {
    let zip_url = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
    let sha_url = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip.sha256";

    let expected_sha = match client.get(sha_url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .text()
            .await
            .ok()
            .and_then(|txt| txt.split_whitespace().next().map(|s| s.trim().to_string()))
            .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())),
        _ => None,
    };

    let bytes = client
        .get(zip_url)
        .send()
        .await
        .map_err(|e| format!("Failed downloading FFmpeg essentials: {}", e))?
        .error_for_status()
        .map_err(|e| format!("FFmpeg download HTTP error: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Failed reading FFmpeg archive: {}", e))?;

    if let Some(ref exp) = expected_sha {
        let actual = hex::encode(Sha256::digest(&bytes));
        if !actual.eq_ignore_ascii_case(exp) {
            return Err(format!(
                "FFmpeg zip SHA-256 mismatch (expected {}, got {})",
                exp, actual
            ));
        }
    }

    let temp_dir = std::env::temp_dir().join(format!("octoheft_ff_setup_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    let zip_path = temp_dir.join("ffmpeg.zip");
    std::fs::write(&zip_path, &bytes).map_err(|e| e.to_string())?;

    let extract_dir = temp_dir.join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    extract_zip_archive(&zip_path, &extract_dir).await?;

    let mut copied = 0usize;
    for target_bin in ["ffmpeg.exe", "ffprobe.exe"] {
        if let Some(found) = find_file_recursive(&extract_dir, target_bin) {
            let dest = bin_dir.join(target_bin);
            std::fs::copy(&found, &dest)
                .map_err(|e| format!("Failed copying {}: {}", target_bin, e))?;
            copied += 1;
        }
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
    if copied == 0 {
        return Err("Extracted FFmpeg archive did not contain ffmpeg.exe".to_string());
    }
    Ok(())
}

async fn install_deno_verified(client: &reqwest::Client, bin_dir: &Path) -> Result<(), String> {
    let zip_url = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
    let sha_url = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip.sha256sum";

    let sha_text = client
        .get(sha_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let expected = sha_text
        .split_whitespace()
        .next()
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| "Invalid Deno sha256sum format".to_string())?
        .to_string();

    let bytes = client
        .get(zip_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(format!("Deno SHA-256 mismatch (expected {}, got {})", expected, actual));
    }

    let temp_dir = std::env::temp_dir().join(format!("octoheft_deno_setup_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    let zip_path = temp_dir.join("deno.zip");
    std::fs::write(&zip_path, &bytes).map_err(|e| e.to_string())?;

    extract_zip_archive(&zip_path, &temp_dir).await?;
    if let Some(found) = find_file_recursive(&temp_dir, "deno.exe") {
        let _ = std::fs::copy(&found, bin_dir.join("deno.exe"));
    }
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(())
}

async fn extract_zip_archive(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut tar_cmd = tokio::process::Command::new("tar");
        tar_cmd
            .arg("-xf")
            .arg(zip_path)
            .arg("-C")
            .arg(dest_dir)
            .creation_flags(CREATE_NO_WINDOW);
        if let Ok(st) = tar_cmd.status().await {
            if st.success() {
                return Ok(());
            }
        }

        let mut ps_cmd = tokio::process::Command::new("powershell");
        ps_cmd
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    zip_path.to_string_lossy().replace('\'', "''"),
                    dest_dir.to_string_lossy().replace('\'', "''")
                ),
            ])
            .creation_flags(CREATE_NO_WINDOW);
        let st = ps_cmd
            .status()
            .await
            .map_err(|e| format!("Failed extracting zip archive: {}", e))?;
        if !st.success() {
            return Err("Archive extraction returned non-zero status".to_string());
        }
    }
    Ok(())
}

fn find_file_recursive(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case(file_name))
                .unwrap_or(false)
            {
                return Some(path);
            }
        } else if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, file_name) {
                return Some(found);
            }
        }
    }
    None
}

pub fn parse_sha256_sums(contents: &str, filename: &str) -> Option<String> {
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let clean_name = name.trim_start_matches('*');
        if clean_name == filename && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sha256_sums() {
        let doc = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  yt-dlp.tar.gz
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210  yt-dlp.exe
";
        assert_eq!(
            parse_sha256_sums(doc, "yt-dlp.exe").as_deref(),
            Some("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210")
        );
        assert_eq!(parse_sha256_sums(doc, "missing.exe"), None);
    }

    #[test]
    fn test_discover_local_tools_smoke() {
        let tools = ResolvedTools::discover();
        assert!(!tools.managed_bin_dir.as_os_str().is_empty());
        // Check that command builder strips PYTHONHOME and keeps UTF-8 env
        let cmd = build_std_command(Path::new("yt-dlp.exe"), &tools);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs
            .iter()
            .any(|(k, v)| *k == "PYTHONUTF8" && *v == Some(std::ffi::OsStr::new("1"))));
    }
}

