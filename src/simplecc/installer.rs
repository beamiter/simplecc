use anyhow::{Result, bail, Context};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::registry::EventTx;

// ═════════════════════════════════════════════════════════
// Platform detection
// ═════════════════════════════════════════════════════════

struct Platform {
    os: &'static str,
    arch: &'static str,
}

fn current_platform() -> Platform {
    Platform {
        os: if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "unknown"
        },
        arch: if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "unknown"
        },
    }
}

// ═════════════════════════════════════════════════════════
// Install directory helpers
// ═════════════════════════════════════════════════════════

fn base_install_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        })
        .join("simplecc/servers")
}

fn server_install_dir(name: &str) -> PathBuf {
    base_install_dir().join(name)
}

/// Returns the path where the binary would be if installed locally.
pub fn installed_binary_path(name: &str) -> Option<PathBuf> {
    let meta = find_server_meta(name)?;
    let plat = current_platform();
    let dir = server_install_dir(name);
    let bin_rel = (meta.binary_rel_path)(&plat);
    Some(dir.join(bin_rel))
}

pub fn is_known_server(name: &str) -> bool {
    find_server_meta(name).is_some()
}

// ═════════════════════════════════════════════════════════
// Server metadata registry
// ═════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
enum ArchiveKind {
    Gz,
    TarGz,
    Zip,
    Command, // subprocess install (go, npm)
}

struct ServerMeta {
    name: &'static str,
    github_repo: Option<&'static str>, // "owner/repo"
    download_url: fn(&Platform, &str) -> String, // (platform, version) -> url
    archive_kind: ArchiveKind,
    binary_rel_path: fn(&Platform) -> String, // relative binary path after extraction
    install_command: Option<fn(&Platform, &Path) -> (String, Vec<String>, Vec<(String, String)>)>,
}

static KNOWN_SERVERS: &[ServerMeta] = &[
    // ── rust-analyzer ──
    ServerMeta {
        name: "rust-analyzer",
        github_repo: Some("rust-lang/rust-analyzer"),
        download_url: |plat, version| {
            let target = match (plat.os, plat.arch) {
                ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
                ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
                ("macos", "x86_64") => "x86_64-apple-darwin",
                ("macos", "aarch64") => "aarch64-apple-darwin",
                _ => "x86_64-unknown-linux-gnu",
            };
            format!(
                "https://github.com/rust-lang/rust-analyzer/releases/download/{}/rust-analyzer-{}.gz",
                version, target
            )
        },
        archive_kind: ArchiveKind::Gz,
        binary_rel_path: |_| "rust-analyzer".to_string(),
        install_command: None,
    },
    // ── clangd ──
    ServerMeta {
        name: "clangd",
        github_repo: Some("clangd/clangd"),
        download_url: |plat, version| {
            let os_str = match plat.os {
                "macos" => "mac",
                _ => "linux",
            };
            format!(
                "https://github.com/clangd/clangd/releases/download/{}/clangd-{}-{}.zip",
                version, os_str, version
            )
        },
        archive_kind: ArchiveKind::Zip,
        binary_rel_path: |_| "bin/clangd".to_string(),
        install_command: None,
    },
    // ── lua-language-server ──
    ServerMeta {
        name: "lua-language-server",
        github_repo: Some("LuaLS/lua-language-server"),
        download_url: |plat, version| {
            let (os_str, arch_str) = match (plat.os, plat.arch) {
                ("linux", "x86_64") => ("linux", "x64"),
                ("linux", "aarch64") => ("linux", "arm64"),
                ("macos", "x86_64") => ("darwin", "x64"),
                ("macos", "aarch64") => ("darwin", "arm64"),
                _ => ("linux", "x64"),
            };
            format!(
                "https://github.com/LuaLS/lua-language-server/releases/download/{}/lua-language-server-{}-{}-{}.tar.gz",
                version, version, os_str, arch_str
            )
        },
        archive_kind: ArchiveKind::TarGz,
        binary_rel_path: |_| "bin/lua-language-server".to_string(),
        install_command: None,
    },
    // ── gopls ──
    ServerMeta {
        name: "gopls",
        github_repo: None,
        download_url: |_, _| String::new(),
        archive_kind: ArchiveKind::Command,
        binary_rel_path: |_| "gopls".to_string(),
        install_command: Some(|_plat, install_dir| {
            (
                "go".to_string(),
                vec!["install".to_string(), "golang.org/x/tools/gopls@latest".to_string()],
                vec![("GOBIN".to_string(), install_dir.to_string_lossy().to_string())],
            )
        }),
    },
    // ── pyright ──
    ServerMeta {
        name: "pyright",
        github_repo: None,
        download_url: |_, _| String::new(),
        archive_kind: ArchiveKind::Command,
        binary_rel_path: |_| "pyright-langserver".to_string(),
        install_command: Some(|_plat, _install_dir| {
            (
                "npm".to_string(),
                vec!["install".to_string(), "-g".to_string(), "pyright".to_string()],
                vec![],
            )
        }),
    },
];

fn find_server_meta(name: &str) -> Option<&'static ServerMeta> {
    KNOWN_SERVERS.iter().find(|s| s.name == name)
}

// ═════════════════════════════════════════════════════════
// Public API
// ═════════════════════════════════════════════════════════

/// Concurrent install guard.
static INSTALLING: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(serde::Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub installed: bool,
    pub path: Option<String>,
}

pub fn list_installable() -> Vec<ServerInfo> {
    KNOWN_SERVERS
        .iter()
        .map(|s| {
            let path = installed_binary_path(s.name);
            let installed = path.as_ref().map(|p| p.exists()).unwrap_or(false);
            ServerInfo {
                name: s.name.to_string(),
                installed,
                path: if installed {
                    path.map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                },
            }
        })
        .collect()
}

pub async fn install_server(name: &str, event_tx: &EventTx) -> Result<PathBuf> {
    let meta = find_server_meta(name)
        .ok_or_else(|| anyhow::anyhow!("unknown server: {}", name))?;

    // Guard against concurrent installs
    {
        let mut set = INSTALLING.lock().await;
        if set.contains(name) {
            bail!("{} is already being installed", name);
        }
        set.insert(name.to_string());
    }

    let result = do_install(meta, event_tx).await;

    // Remove from installing set
    {
        let mut set = INSTALLING.lock().await;
        set.remove(name);
    }

    result
}

// ═════════════════════════════════════════════════════════
// Internal install logic
// ═════════════════════════════════════════════════════════

async fn do_install(meta: &ServerMeta, event_tx: &EventTx) -> Result<PathBuf> {
    let plat = current_platform();
    let install_dir = server_install_dir(meta.name);

    // Create install directory
    tokio::fs::create_dir_all(&install_dir).await
        .context("failed to create install directory")?;

    match meta.archive_kind {
        ArchiveKind::Command => {
            install_via_command(meta, &plat, &install_dir, event_tx).await?;
        }
        _ => {
            install_via_download(meta, &plat, &install_dir, event_tx).await?;
        }
    }

    let bin_path = install_dir.join((meta.binary_rel_path)(&plat));

    if !bin_path.exists() {
        // Try to find the binary in subdirectories (clangd extracts into clangd_*/bin/)
        if let Some(found) = find_binary_recursive(&install_dir, &bin_path.file_name().unwrap().to_string_lossy()).await {
            return Ok(found);
        }
        bail!("binary not found after installation: {}", bin_path.display());
    }

    Ok(bin_path)
}

async fn install_via_download(
    meta: &ServerMeta,
    plat: &Platform,
    install_dir: &Path,
    event_tx: &EventTx,
) -> Result<()> {
    // Get latest version
    send_progress(event_tx, meta.name, "checking latest version", 0).await;

    let version = if let Some(repo) = meta.github_repo {
        fetch_latest_github_version(repo).await?
    } else {
        bail!("no github repo for {}", meta.name);
    };

    eprintln!("[simplecc] {} latest version: {}", meta.name, version);

    let url = (meta.download_url)(plat, &version);
    eprintln!("[simplecc] downloading from: {}", url);

    // Download
    let archive_ext = match meta.archive_kind {
        ArchiveKind::Gz => ".gz",
        ArchiveKind::TarGz => ".tar.gz",
        ArchiveKind::Zip => ".zip",
        ArchiveKind::Command => unreachable!(),
    };
    let tmp_file = install_dir.join(format!("download{}", archive_ext));

    download_with_progress(&url, &tmp_file, event_tx, meta.name).await?;

    // Extract
    send_progress(event_tx, meta.name, "extracting", 0).await;

    let tmp_file_clone = tmp_file.clone();
    let install_dir_owned = install_dir.to_path_buf();
    let archive_kind = meta.archive_kind;
    let bin_name = (meta.binary_rel_path)(plat);

    tokio::task::spawn_blocking(move || -> Result<()> {
        match archive_kind {
            ArchiveKind::Gz => extract_gz(&tmp_file_clone, &install_dir_owned.join(&bin_name))?,
            ArchiveKind::TarGz => extract_tar_gz(&tmp_file_clone, &install_dir_owned)?,
            ArchiveKind::Zip => extract_zip(&tmp_file_clone, &install_dir_owned)?,
            ArchiveKind::Command => unreachable!(),
        }
        Ok(())
    })
    .await??;

    // Clean up archive
    let _ = tokio::fs::remove_file(&tmp_file).await;

    // Set executable permissions
    let bin_path = install_dir.join((meta.binary_rel_path)(plat));
    if bin_path.exists() {
        set_executable(&bin_path)?;
    }

    send_progress(event_tx, meta.name, "done", 100).await;
    Ok(())
}

async fn install_via_command(
    meta: &ServerMeta,
    plat: &Platform,
    install_dir: &Path,
    event_tx: &EventTx,
) -> Result<()> {
    let make_cmd = meta.install_command
        .ok_or_else(|| anyhow::anyhow!("no install command for {}", meta.name))?;

    let (cmd, args, envs) = make_cmd(plat, install_dir);

    send_progress(event_tx, meta.name, &format!("running {} ...", cmd), 0).await;

    // Check if the command tool exists
    if which::which(&cmd).is_err() {
        bail!("'{}' not found in PATH. Please install it first.", cmd);
    }

    let mut command = tokio::process::Command::new(&cmd);
    command.args(&args);
    for (k, v) in &envs {
        command.env(k, v);
    }

    let output = command.output().await
        .context(format!("failed to run {}", cmd))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} failed: {}", cmd, stderr);
    }

    send_progress(event_tx, meta.name, "done", 100).await;
    Ok(())
}

// ═════════════════════════════════════════════════════════
// GitHub API
// ═════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
}

async fn fetch_latest_github_version(repo: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);

    let client = reqwest::Client::builder()
        .user_agent("simplecc-vim-plugin")
        .build()?;

    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("GitHub API error {}: {}", status, body);
    }

    let release: GithubRelease = resp.json().await?;
    Ok(release.tag_name)
}

// ═════════════════════════════════════════════════════════
// Download with progress
// ═════════════════════════════════════════════════════════

async fn download_with_progress(
    url: &str,
    dest: &Path,
    event_tx: &EventTx,
    server_name: &str,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("simplecc-vim-plugin")
        .build()?;

    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        bail!("download failed: HTTP {}", resp.status());
    }

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();

    let mut file = tokio::fs::File::create(dest).await?;
    let mut last_percent: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if total > 0 {
            let percent = (downloaded * 100) / total;
            // Report progress every 5%
            if percent >= last_percent + 5 {
                last_percent = percent;
                send_progress(event_tx, server_name, "downloading", percent).await;
            }
        }
    }

    file.flush().await?;
    Ok(())
}

// ═════════════════════════════════════════════════════════
// Archive extraction
// ═════════════════════════════════════════════════════════

fn extract_gz(src: &Path, dest: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let file = std::fs::File::open(src)?;
    let mut decoder = GzDecoder::new(file);
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(&mut decoder, &mut out)?;

    // Also try reading any trailing data (some .gz files are fine)
    let mut buf = Vec::new();
    let _ = decoder.read_to_end(&mut buf);

    set_executable(dest)?;
    Ok(())
}

fn extract_tar_gz(src: &Path, dest_dir: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = std::fs::File::open(src)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(dest_dir)?;
    Ok(())
}

fn extract_zip(src: &Path, dest_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();

        // Strip top-level directory (e.g. clangd_18.1.3/ -> "")
        let rel_path = strip_top_dir(&name);
        if rel_path.is_empty() {
            continue;
        }

        let out_path = dest_dir.join(rel_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;

            // Preserve executable bit
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
    }

    Ok(())
}

/// Strip the top-level directory from a zip entry path.
/// "clangd_18.1.3/bin/clangd" -> "bin/clangd"
fn strip_top_dir(path: &str) -> &str {
    if let Some(pos) = path.find('/') {
        &path[pos + 1..]
    } else {
        path
    }
}

// ═════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

async fn send_progress(event_tx: &EventTx, server: &str, stage: &str, percent: u64) {
    let ev = json!({
        "type": "installProgress",
        "server": server,
        "stage": stage,
        "percent": percent,
    });
    let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
}

/// Recursively search for a binary by filename.
async fn find_binary_recursive(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = Box::pin(find_binary_recursive(&path, name)).await {
                return Some(found);
            }
        } else if path.file_name().map(|f| f.to_string_lossy() == name).unwrap_or(false) {
            set_executable(&path).ok();
            return Some(path);
        }
    }
    None
}
