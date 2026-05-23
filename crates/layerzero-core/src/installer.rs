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
            "--model".to_string(), model_path.to_string_lossy().to_string(),
            "--port".to_string(), port.to_string(),
            "--ctx-size".to_string(), context_length.to_string(),
            "--parallel".to_string(), "4".to_string(),
            "--log-disable".to_string(),
        ];
        if embedding {
            args.push("--embedding".to_string());
        }

        let child = Command::new(&server_bin).args(&args).spawn()?;

        std::thread::sleep(std::time::Duration::from_secs(2));
        Ok(Self { child: Some(child), port })
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
        "llama-server not found in {}. Run `layerzero install llama` to install.",
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
    let client = Client::builder().user_agent("layerzero/0.1.0").build()?;

    info!("fetching latest llama.cpp release...");
    let url = format!("https://api.github.com/repos/{}/releases/latest", LLAMA_CPP_REPO);
    let release: GithubRelease = client.get(&url).send().await?.json().await?;
    info!("latest: {}", release.tag_name);

    let asset = find_platform_asset(&release.assets)?;
    info!("downloading {} ({:.1} MB)", asset.name, asset.size as f64 / 1_048_576.0);

    let bytes = download_bytes(&client, &asset.browser_download_url).await?;
    std::fs::create_dir_all(&config.bin_dir)?;
    extract_archive(&bytes, &asset.name, &config.bin_dir)?;

    info!("llama.cpp installed to {}", config.bin_dir.display());
    Ok(config.bin_dir.clone())
}

fn find_platform_asset(assets: &[GithubAsset]) -> Result<&GithubAsset> {
    let (os, arch) = (
        if cfg!(target_os = "windows") { "win" } else if cfg!(target_os = "macos") { "macos" } else { "ubuntu" },
        if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" },
    );

    let patterns: Vec<String> = if cfg!(target_os = "windows") {
        vec![format!("bin-{}-avx2-{}", os, arch), format!("bin-{}-{}", os, arch)]
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

    assets.iter().find(|a| {
        let l = a.name.to_lowercase();
        l.contains(os) && (l.ends_with(".zip") || l.ends_with(".tar.gz"))
    })
    .ok_or_else(|| anyhow!("no suitable llama.cpp binary for {}/{}", os, arch))
}

pub(crate) async fn download_bytes(client: &Client, url: &str) -> Result<bytes::Bytes> {
    let mut resp = client.get(url).send().await?;
    let total = resp.content_length().unwrap_or(0);
    let mut downloaded = 0u64;
    let mut chunks = Vec::new();

    while let Some(chunk) = resp.chunk().await? {
        downloaded += chunk.len() as u64;
        if total > 0 && downloaded % (10 * 1_048_576) < chunk.len() as u64 {
            info!("  {:.0}%", downloaded as f64 / total as f64 * 100.0);
        }
        chunks.push(chunk);
    }

    Ok(chunks.concat().into())
}

fn extract_archive(bytes: &[u8], filename: &str, dest: &Path) -> Result<()> {
    let is_server = |name: &str| {
        name.contains("llama-server") || name.contains("server")
            || name.ends_with(".dll") || name.ends_with(".so") || name.ends_with(".dylib")
    };

    if filename.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name().to_string();
            if is_server(&name) {
                let out = dest.join(Path::new(&name).file_name().unwrap_or_default());
                let mut f = std::fs::File::create(&out)?;
                std::io::copy(&mut file, &mut f)?;
                set_executable(&out);
                info!("extracted: {}", out.display());
            }
        }
    } else if filename.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(gz);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if is_server(&path.to_string_lossy()) {
                let out = dest.join(path.file_name().unwrap_or_default());
                entry.unpack(&out)?;
                set_executable(&out);
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
    let client = Client::builder().user_agent("layerzero/0.1.0").build()?;
    let url = format!("https://huggingface.co/api/models/{}", repo);
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let info: HfModelInfo = req.send().await?.json().await?;
    Ok(info.siblings)
}

pub async fn download_hf_model(
    config: &InstallerConfig,
    repo: &str,
    filename: &str,
    token: Option<&str>,
) -> Result<PathBuf> {
    let client = Client::builder()
        .user_agent("layerzero/0.1.0")
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

    let mut file = std::fs::File::create(&model_path)?;
    let mut stream = resp.bytes_stream();
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded += chunk.len() as u64;
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
        .filter(|p| p.extension().map(|x| x == "gguf" || x == "bin").unwrap_or(false))
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
    client.get(url).send().await.map(|r| r.status().is_success()).unwrap_or(false)
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
        let model_path = config.installer.models_dir.join(&config.installer.embedding_file);
        ensure_model(config, &model_path, &config.installer.embedding_repo, &config.installer.embedding_file).await?;

        if llama_healthy(&config.llm.base_url).await {
            info!("embeddings backend already running at {}", config.llm.base_url);
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
        let model_path = config.installer.models_dir.join(&config.installer.chat_file);
        ensure_model(config, &model_path, &config.installer.chat_repo, &config.installer.chat_file).await?;

        if llama_healthy(&chat_url).await {
            info!("local chat backend already running at {}", chat_url);
        } else {
            info!("no remote chat key set — starting local chat fallback ({})", config.installer.chat_file);
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
    download_hf_model(&config.installer, repo, file, config.installer.hf_token.as_deref()).await?;
    Ok(())
}
