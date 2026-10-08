use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use tracing::info;

use crate::config::InstallerConfig;

const LLAMA_CPP_REPO: &str = "ggml-org/llama.cpp";
const HF_API_BASE: &str = "https://huggingface.co";

pub struct LlamaServer {
    child: Option<Child>,
    pub port: u16,
}

impl LlamaServer {
    pub fn start(
        config: &InstallerConfig,
        model_path: &Path,
        port: u16,
        context_length: u32,
        embedding: bool,
    ) -> Result<Self> {
        let server_bin = find_llama_server(&config.bin_dir)?;
        info!(
            "starting llama-server ({}) on port {}",
            if embedding { "embeddings" } else { "chat" },
            port
        );

        let mut args = vec![
            "--model".to_string(),
            model_path.to_string_lossy().to_string(),
            "--port".to_string(),
            port.to_string(),
            "--ctx-size".to_string(),
            context_length.to_string(),
            "--parallel".to_string(),
            "4".to_string(),
            "--log-disable".to_string(),
        ];
        if embedding {
            args.push("--embedding".to_string());
        }

        let child = Command::new(&server_bin).args(&args).spawn()?;

        std::thread::sleep(std::time::Duration::from_secs(2));
        Ok(Self {
            child: Some(child),
            port,
        })
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn find_llama_server(bin_dir: &Path) -> Result<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["llama-server.exe", "server.exe"]
    } else {
        &["llama-server", "server"]
    };

    for name in names {
        let p = bin_dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }

    Err(anyhow!(
        "llama-server not found in {}. Run `layer0 install llama` to install.",
        bin_dir.display()
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct GithubRelease {
    pub(crate) tag_name: String,
    pub(crate) assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GithubAsset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
}

pub async fn install_llama_cpp(config: &InstallerConfig) -> Result<PathBuf> {
    let _slot = download_slot()?;
    let client = Client::builder()
        .user_agent("layer0/0.1.0")
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    info!("fetching latest llama.cpp release...");
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        LLAMA_CPP_REPO
    );
    let release: GithubRelease = client.get(&url).send().await?.json().await?;
    info!("latest: {}", release.tag_name);

    let asset = find_platform_asset(&release.assets)?;
    info!(
        "downloading {} ({:.1} MB)",
        asset.name,
        asset.size as f64 / 1_048_576.0
    );

    let bytes = download_bytes(&client, &asset.browser_download_url).await?;
    std::fs::create_dir_all(&config.bin_dir)?;
    extract_archive(&bytes, &asset.name, &config.bin_dir)?;

    info!("llama.cpp installed to {}", config.bin_dir.display());
    Ok(config.bin_dir.clone())
}

fn find_platform_asset(assets: &[GithubAsset]) -> Result<&GithubAsset> {
    let (os, arch) = (
        if cfg!(target_os = "windows") {
            "win"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "ubuntu"
        },
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        },
    );

    let patterns: Vec<String> = if cfg!(target_os = "windows") {
        vec![
            format!("bin-{}-avx2-{}", os, arch),
            format!("bin-{}-{}", os, arch),
        ]
    } else {
        vec![format!("bin-{}-{}", os, arch)]
    };

    for pat in &patterns {
        if let Some(a) = assets.iter().find(|a| {
            let l = a.name.to_lowercase();
            l.contains(pat.as_str()) && (l.ends_with(".zip") || l.ends_with(".tar.gz"))
        }) {
            return Ok(a);
        }
    }

    assets
        .iter()
        .find(|a| {
            let l = a.name.to_lowercase();
            l.contains(os) && (l.ends_with(".zip") || l.ends_with(".tar.gz"))
        })
        .ok_or_else(|| anyhow!("no suitable llama.cpp binary for {}/{}", os, arch))
}

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) async fn download_bytes(client: &Client, url: &str) -> Result<bytes::Bytes> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= MAX_ARCHIVE_BYTES,
        "Archive exceeds download limit"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            (bytes.len() as u64).saturating_add(chunk.len() as u64) <= MAX_ARCHIVE_BYTES,
            "Archive exceeds download limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.into())
}

pub(crate) fn validate_tar_archive(bytes: &[u8]) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);
    let mut total = 0u64;
    for (index, entry) in archive.entries()?.raw(true).enumerate() {
        anyhow::ensure!(index < 4096, "Too many archive entries");
        let entry = entry?;
        let kind = entry.header().entry_type();
        let size = entry.size();
        let metadata = kind.is_gnu_longname()
            || kind.is_pax_local_extensions()
            || kind.is_pax_global_extensions();
        anyhow::ensure!(
            kind.is_file() || kind.is_dir() || metadata,
            "Archive links are unsupported"
        );
        anyhow::ensure!(
            size <= if metadata {
                64 * 1024
            } else {
                MAX_BINARY_BYTES
            },
            "Archive entry exceeds size limit"
        );
        total = total
            .checked_add(size)
            .ok_or_else(|| anyhow!("Archive size overflow"))?;
        anyhow::ensure!(
            total <= MAX_EXTRACTED_BYTES,
            "Archive exceeds extraction limit"
        );
    }
    Ok(())
}

pub(crate) fn copy_binary(
    reader: &mut impl std::io::Read,
    path: &Path,
    declared: u64,
) -> Result<()> {
    use std::io::Read;
    anyhow::ensure!(
        declared <= MAX_BINARY_BYTES,
        "Binary exceeds extraction limit"
    );
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Missing binary directory"))?;
    let temporary = parent.join(format!(".extract-{}", uuid::Uuid::new_v4()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(temporary.clone());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(&temporary)?;
    let actual = std::io::copy(&mut reader.take(MAX_BINARY_BYTES + 1), &mut file)?;
    anyhow::ensure!(
        actual <= MAX_BINARY_BYTES && actual == declared,
        "Invalid extracted binary size"
    );
    file.sync_all()?;
    drop(file);
    set_executable(&temporary);
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn extract_archive(bytes: &[u8], filename: &str, dest: &Path) -> Result<()> {
    let mut extracted = 0u64;
    let mut entries = 0usize;
    let is_server = |name: &str| {
        name.contains("llama-server")
            || name.contains("server")
            || name.ends_with(".dll")
            || name.ends_with(".so")
            || name.ends_with(".dylib")
    };

    if filename.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        anyhow::ensure!(archive.len() <= 4096, "Too many archive entries");
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name().to_string();
            anyhow::ensure!(
                file.unix_mode()
                    .is_none_or(|mode| mode & 0o170000 != 0o120000),
                "Archive links are unsupported"
            );
            extracted = extracted
                .checked_add(file.size())
                .ok_or_else(|| anyhow!("Archive size overflow"))?;
            anyhow::ensure!(
                extracted <= MAX_EXTRACTED_BYTES,
                "Archive exceeds extraction limit"
            );
            if is_server(&name) {
                let out = dest.join(Path::new(&name).file_name().unwrap_or_default());
                let declared = file.size();
                copy_binary(&mut file, &out, declared)?;
                info!("extracted: {}", out.display());
            }
        }
    } else if filename.ends_with(".tar.gz") {
        validate_tar_archive(bytes)?;
        let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(gz);
        for entry in archive.entries()? {
            let mut entry = entry?;
            entries += 1;
            anyhow::ensure!(entries <= 4096, "Too many archive entries");
            anyhow::ensure!(
                entry.header().entry_type().is_file() || entry.header().entry_type().is_dir(),
                "Archive links are unsupported"
            );
            extracted = extracted
                .checked_add(entry.size())
                .ok_or_else(|| anyhow!("Archive size overflow"))?;
            anyhow::ensure!(
                extracted <= MAX_EXTRACTED_BYTES,
                "Archive exceeds extraction limit"
            );
            let path = entry.path()?.to_path_buf();
            if is_server(&path.to_string_lossy()) {
                let out = dest.join(path.file_name().unwrap_or_default());
                let declared = entry.size();
                copy_binary(&mut entry, &out, declared)?;
                info!("extracted: {}", out.display());
            }
        }
    } else {
        return Err(anyhow!("unsupported archive: {}", filename));
    }

    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {}

#[derive(Debug, Serialize, Deserialize)]
pub struct HfModelFile {
    pub rfilename: String,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HfModelInfo {
    pub id: String,
    pub siblings: Vec<HfModelFile>,
}

pub async fn list_hf_model_files(repo: &str, token: Option<&str>) -> Result<Vec<HfModelFile>> {
    let client = Client::builder().user_agent("layer0/0.1.0").build()?;
    let url = format!("https://huggingface.co/api/models/{}", repo);
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let info: HfModelInfo = req.send().await?.json().await?;
    Ok(info.siblings)
}

fn validate_hf_path(repo: &str, filename: &str) -> Result<()> {
    let safe_segment = |segment: &str| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.len() <= 255
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let repository: Vec<_> = repo.split('/').collect();
    anyhow::ensure!(
        repository.len() == 2 && repository.iter().all(|s| safe_segment(s)),
        "Expected Hugging Face owner/repository"
    );
    anyhow::ensure!(
        filename.len() <= 1024 && filename.split('/').all(safe_segment),
        "Invalid model filename"
    );
    Ok(())
}

fn download_slot() -> Result<tokio::sync::SemaphorePermit<'static>> {
    static DOWNLOAD_SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    DOWNLOAD_SLOTS
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .try_acquire()
        .map_err(|_| anyhow!("model download capacity exhausted"))
}

pub async fn download_hf_model(
    config: &InstallerConfig,
    repo: &str,
    filename: &str,
    token: Option<&str>,
) -> Result<PathBuf> {
    validate_hf_path(repo, filename)?;
    let _slot = download_slot()?;
    let client = Client::builder()
        .user_agent("layer0/0.1.0")
        .timeout(std::time::Duration::from_secs(3600))
        .build()?;

    let url = format!("{}/{}/resolve/main/{}", HF_API_BASE, repo, filename);
    info!("downloading {} from {}", filename, repo);

    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("HuggingFace download failed: {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(0);
    let model_path = config.models_dir.join(filename);
    std::fs::create_dir_all(&config.models_dir)?;

    let directory =
        cap_std::fs::Dir::open_ambient_dir(&config.models_dir, cap_std::ambient_authority())?;
    if let Some(parent) = Path::new(filename)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        directory.create_dir_all(parent)?;
    }
    let temporary = format!(".download-{}.tmp", uuid::Uuid::new_v4());
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = directory.open_with(&temporary, &options)?;
    struct Cleanup<'a>(&'a cap_std::fs::Dir, String);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.remove_file(&self.1);
        }
    }
    let _cleanup = Cleanup(&directory, temporary.clone());
    const MAX_MODEL_BYTES: u64 = 128 * 1024 * 1024 * 1024;
    anyhow::ensure!(
        total <= MAX_MODEL_BYTES,
        "Model exceeds download size limit"
    );
    let mut stream = resp.bytes_stream();
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| anyhow!("Model size overflow"))?;
        anyhow::ensure!(
            downloaded <= MAX_MODEL_BYTES,
            "Model exceeds download size limit"
        );
        if total > 0 && downloaded % (100 * 1_048_576) < chunk.len() as u64 {
            info!(
                "  {:.1} / {:.1} GB ({:.0}%)",
                downloaded as f64 / 1_073_741_824.0,
                total as f64 / 1_073_741_824.0,
                downloaded as f64 / total as f64 * 100.0
            );
        }
        use std::io::Write;
        file.write_all(&chunk)?;
    }

    file.sync_all()?;
    drop(file);
    directory.rename(&temporary, &directory, filename)?;
    info!("saved to {}", model_path.display());
    Ok(model_path)
}

pub fn list_installed_models(models_dir: &Path) -> Result<Vec<PathBuf>> {
    if !models_dir.exists() {
        return Ok(vec![]);
    }
    Ok(std::fs::read_dir(models_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|x| x == "gguf" || x == "bin")
                .unwrap_or(false)
        })
        .collect())
}

async fn llama_healthy(base_url: &str) -> bool {
    let url = format!("{}/health", base_url.trim_end_matches('/'));
    let client = match Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn wait_for_health(base_url: &str, max_secs: u64) -> bool {
    for _ in 0..max_secs {
        if llama_healthy(base_url).await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    false
}

/// Frictionless startup: ensure llama.cpp + the needed default models are
/// present, and start the local sidecar(s). Returns handles to the managed
/// children (kept alive by the caller). Starts:
/// - an embeddings sidecar (nomic) when embeddings are configured locally;
/// - a chat sidecar (gemma fallback) when no remote chat key is set.
/// Returns an empty list when auto-start is disabled.
pub async fn ensure_ready(config: &crate::config::Config) -> Result<Vec<LlamaServer>> {
    if !config.installer.auto_start {
        return Ok(vec![]);
    }

    let want_embeddings = crate::config::is_local_url(&config.llm.base_url);
    let want_chat = config.chat_uses_local_fallback();

    if !want_embeddings && !want_chat {
        return Ok(vec![]);
    }

    if find_llama_server(&config.installer.bin_dir).is_err() {
        info!("llama.cpp not found — installing...");
        install_llama_cpp(&config.installer).await?;
    }

    let mut servers = Vec::new();

    if want_embeddings {
        let model_path = config
            .installer
            .models_dir
            .join(&config.installer.embedding_file);
        ensure_model(
            config,
            &model_path,
            &config.installer.embedding_repo,
            &config.installer.embedding_file,
        )
        .await?;

        if llama_healthy(&config.llm.base_url).await {
            info!(
                "embeddings backend already running at {}",
                config.llm.base_url
            );
        } else {
            let s = LlamaServer::start(
                &config.installer,
                &model_path,
                config.installer.llama_server_port,
                config.llm.context_length,
                true,
            )?;
            if wait_for_health(&config.llm.base_url, 30).await {
                info!("embeddings backend ready at {}", config.llm.base_url);
            } else {
                info!("embeddings backend starting at {}", config.llm.base_url);
            }
            servers.push(s);
        }
    }

    if want_chat {
        let chat_url = format!("http://127.0.0.1:{}", config.installer.chat_server_port);
        let model_path = config
            .installer
            .models_dir
            .join(&config.installer.chat_file);
        ensure_model(
            config,
            &model_path,
            &config.installer.chat_repo,
            &config.installer.chat_file,
        )
        .await?;

        if llama_healthy(&chat_url).await {
            info!("local chat backend already running at {}", chat_url);
        } else {
            info!(
                "no remote chat key set — starting local chat fallback ({})",
                config.installer.chat_file
            );
            let s = LlamaServer::start(
                &config.installer,
                &model_path,
                config.installer.chat_server_port,
                2048,
                false,
            )?;
            if wait_for_health(&chat_url, 60).await {
                info!("local chat backend ready at {}", chat_url);
            } else {
                info!("local chat backend starting at {}", chat_url);
            }
            servers.push(s);
        }
    }

    Ok(servers)
}

async fn ensure_model(
    config: &crate::config::Config,
    model_path: &Path,
    repo: &str,
    file: &str,
) -> Result<()> {
    if model_path.exists() {
        return Ok(());
    }
    info!("downloading default model {}...", file);
    download_hf_model(
        &config.installer,
        repo,
        file,
        config.installer.hf_token.as_deref(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod path_security_tests {
    use super::*;
    #[test]
    fn extracted_size_is_checked_before_replacing_an_existing_binary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("llama-server");
        std::fs::write(&path, b"existing").unwrap();
        let payload = vec![42; 1024 * 1024];
        assert!(copy_binary(&mut payload.as_slice(), &path, 1).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(copy_binary(&mut b"new".as_slice(), &path, MAX_BINARY_BYTES + 1).is_err());
        copy_binary(&mut b"new".as_slice(), &path, 3).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn tar_metadata_budget_is_checked_before_parser_allocation() {
        use std::io::Write;
        let mut header = tar::Header::new_gnu();
        header.set_path("long-name").unwrap();
        header.set_entry_type(tar::EntryType::GNULongName);
        header.set_size(64 * 1024 + 1);
        header.set_cksum();
        let mut compressed =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        compressed.write_all(header.as_bytes()).unwrap();
        let error = validate_tar_archive(&compressed.finish().unwrap()).unwrap_err();
        assert!(error.to_string().contains("entry exceeds size limit"));

        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "bin/llama-server", b"new".as_slice())
            .unwrap();
        let mut compressed =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        compressed
            .write_all(&archive.into_inner().unwrap())
            .unwrap();
        let bytes = compressed.finish().unwrap();
        let directory = tempfile::tempdir().unwrap();
        extract_archive(&bytes, "release.tar.gz", directory.path()).unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("llama-server")).unwrap(),
            b"new"
        );
    }
    #[test]
    fn model_paths_reject_escape_and_url_delimiters() {
        assert!(validate_hf_path("owner/repo", "sub/model-Q4.gguf").is_ok());
        for name in [
            "../victim",
            "/tmp/victim",
            "C:\\victim",
            "a/../../victim",
            "a?x",
            "a#x",
            "a\\b",
        ] {
            assert!(validate_hf_path("owner/repo", name).is_err());
        }
        for repo in ["owner/repo#", "owner/repo?", "owner/../repo", "/owner/repo"] {
            assert!(validate_hf_path(repo, "model.gguf").is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn capability_directory_cannot_publish_through_an_escaping_parent_link() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("nested")).unwrap();
        let dir =
            cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
        dir.write("temporary", "model").unwrap();
        assert!(dir.rename("temporary", &dir, "nested/victim").is_err());
        assert!(!outside.path().join("victim").exists());
        dir.write("victim", "old").unwrap();
        dir.rename("temporary", &dir, "victim").unwrap();
        assert_eq!(dir.read("victim").unwrap(), b"model");
    }
}
