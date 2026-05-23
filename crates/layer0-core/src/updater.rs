use anyhow::{anyhow, Result};
use reqwest::Client;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::config::UpdateConfig;
use crate::installer::{download_bytes, GithubRelease};

const BINARIES: &[&str] = &["layer0", "layer0-server", "layer0-mcp"];

#[derive(Debug)]
pub enum UpdateOutcome {
    UpToDate { version: String },
    Updated { from: String, to: String },
}

/// The release-asset target triple for the running platform. Must match the
/// triple embedded in release archive names by the release workflow.
pub fn current_target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return None,
    })
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let v = v.trim().trim_start_matches('v');
    let mut it = v.split(['.', '-', '+']).filter_map(|p| p.parse::<u64>().ok());
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn is_newer(remote: &str, local: &str) -> bool {
    parse_version(remote) > parse_version(local)
}

async fn fetch_latest(repo: &str) -> Result<GithubRelease> {
    let client = Client::builder()
        .user_agent(format!("layer0/{}", current_version()))
        .build()?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let release: GithubRelease = client.get(&url).send().await?.error_for_status()?.json().await?;
    Ok(release)
}

/// Returns the newer version string if a newer release is available.
pub async fn check_latest(cfg: &UpdateConfig) -> Result<Option<String>> {
    let release = fetch_latest(&cfg.repo).await?;
    if is_newer(&release.tag_name, current_version()) {
        Ok(Some(release.tag_name))
    } else {
        Ok(None)
    }
}

/// Check for, download, and apply the latest release. Replacement of the
/// currently running binary takes effect on the next start.
pub async fn update_now(cfg: &UpdateConfig) -> Result<UpdateOutcome> {
    let target = current_target().ok_or_else(|| anyhow!("unsupported platform for self-update"))?;
    let release = fetch_latest(&cfg.repo).await?;
    let local = current_version().to_string();

    if !is_newer(&release.tag_name, &local) {
        return Ok(UpdateOutcome::UpToDate { version: local });
    }

    let ext = if cfg!(windows) { ".zip" } else { ".tar.gz" };
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.contains(target) && a.name.ends_with(ext))
        .ok_or_else(|| anyhow!("no release asset for {} ({})", target, ext))?;

    info!("downloading update {} ({:.1} MB)", asset.name, asset.size as f64 / 1_048_576.0);
    let client = Client::builder()
        .user_agent(format!("layer0/{}", current_version()))
        .build()?;
    let bytes = download_bytes(&client, &asset.browser_download_url).await?;

    let staging = std::env::temp_dir().join(format!("layer0-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging)?;
    let extracted = extract_binaries(&bytes, &asset.name, &staging)?;
    if extracted.is_empty() {
        return Err(anyhow!("update archive contained no layer0 binaries"));
    }

    apply_binaries(&extracted)?;
    let _ = std::fs::remove_dir_all(&staging);

    Ok(UpdateOutcome::Updated { from: local, to: release.tag_name })
}

fn bin_filename(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn is_target_binary(entry_name: &str) -> Option<String> {
    let base = Path::new(entry_name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())?;
    for b in BINARIES {
        if base == bin_filename(b) {
            return Some(base);
        }
    }
    None
}

fn extract_binaries(bytes: &[u8], archive_name: &str, dest: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();

    if archive_name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            let name = file.name().to_string();
            if let Some(base) = is_target_binary(&name) {
                let path = dest.join(&base);
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                std::fs::write(&path, &buf)?;
                out.push(path);
            }
        }
    } else if archive_name.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut tar = tar::Archive::new(gz);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let name = entry.path()?.to_string_lossy().to_string();
            if let Some(base) = is_target_binary(&name) {
                let path = dest.join(&base);
                entry.unpack(&path)?;
                out.push(path);
            }
        }
    } else {
        return Err(anyhow!("unsupported archive type: {}", archive_name));
    }

    Ok(out)
}

fn apply_binaries(extracted: &[PathBuf]) -> Result<()> {
    let current = std::env::current_exe()?;
    let install_dir = current
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve install directory"))?
        .to_path_buf();
    let current_name = current.file_name().map(|s| s.to_string_lossy().to_string());

    for src in extracted {
        let name = src.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let dest = install_dir.join(&name);

        if Some(&name) == current_name.as_ref() {
            // Replace the running executable safely (rename-and-swap under the hood).
            self_replace::self_replace(src)?;
            info!("updated {} (running binary; restart to apply)", name);
        } else {
            match std::fs::copy(src, &dest) {
                Ok(_) => info!("updated {}", name),
                Err(e) => warn!(
                    "could not replace {} (in use? stop it and re-run `layer0 update`): {}",
                    dest.display(),
                    e
                ),
            }
        }
    }

    Ok(())
}
