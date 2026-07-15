use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::registry::EventTx;

const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;

// ═════════════════════════════════════════════════════════
// Platform detection
// ═════════════════════════════════════════════════════════

struct Platform {
    os: &'static str,
    arch: &'static str,
}

impl Platform {
    fn ensure_supported(&self) -> Result<()> {
        if !matches!(self.os, "linux" | "macos") {
            bail!(
                "managed server installation is not supported on {}",
                self.os
            );
        }
        if !matches!(self.arch, "x86_64" | "aarch64") {
            bail!(
                "managed server installation is not supported on {} architecture",
                self.arch
            );
        }
        Ok(())
    }
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
    // Julia LSP lives in a shared named environment, not a managed binary dir.
    // Its "installed" marker is the env's Project.toml.
    if name == "julia-lsp" {
        return Some(julia_lsp_env_project());
    }
    let meta = find_server_meta(name)?;
    let plat = current_platform();
    let dir = server_install_dir(name);
    let bin_rel = (meta.binary_rel_path)(&plat);
    Some(dir.join(bin_rel))
}

/// First Julia depot directory (respects JULIA_DEPOT_PATH, defaults to ~/.julia).
fn julia_depot() -> PathBuf {
    if let Some(dp) = std::env::var_os("JULIA_DEPOT_PATH") {
        let s = dp.to_string_lossy();
        if let Some(first) = s.split(':').find(|p| !p.is_empty()) {
            return PathBuf::from(first);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".julia")
}

/// Project.toml of the dedicated `@simplecc` named environment.
fn julia_lsp_env_project() -> PathBuf {
    julia_depot()
        .join("environments")
        .join("simplecc")
        .join("Project.toml")
}

/// Whether LanguageServer.jl is installed in the `@simplecc` named environment.
pub fn is_julia_lsp_installed() -> bool {
    match std::fs::read_to_string(julia_lsp_env_project()) {
        Ok(content) => content.contains("LanguageServer"),
        Err(_) => false,
    }
}

pub fn is_known_server(name: &str) -> bool {
    find_server_meta(name).is_some()
}

/// A managed install is usable only when its expected marker is complete. This
/// deliberately rejects empty/non-executable files left by older interrupted
/// installers instead of reporting them as installed.
pub fn is_server_installed(name: &str) -> bool {
    if name == "julia-lsp" {
        return is_julia_lsp_installed();
    }
    let Some(path) = installed_binary_path(name) else {
        return false;
    };
    is_usable_executable(&path)
}

fn is_usable_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
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
    github_repo: Option<&'static str>,           // "owner/repo"
    download_url: fn(&Platform, &str) -> String, // (platform, version) -> url
    archive_kind: ArchiveKind,
    binary_rel_path: fn(&Platform) -> String, // relative binary path after extraction
    install_command: Option<InstallCommandBuilder>,
}

type InstallCommandBuilder = fn(&Platform, &Path) -> InstallCommand;

struct InstallCommand {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
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
        install_command: Some(|_plat, install_dir| InstallCommand {
            program: "go".to_string(),
            args: vec![
                "install".to_string(),
                "golang.org/x/tools/gopls@latest".to_string(),
            ],
            env: vec![(
                "GOBIN".to_string(),
                install_dir.to_string_lossy().to_string(),
            )],
        }),
    },
    // ── pyright ──
    ServerMeta {
        name: "pyright",
        github_repo: None,
        download_url: |_, _| String::new(),
        archive_kind: ArchiveKind::Command,
        binary_rel_path: |_| "node_modules/.bin/pyright-langserver".to_string(),
        install_command: Some(|_plat, install_dir| InstallCommand {
            program: "npm".to_string(),
            args: vec![
                "install".to_string(),
                "--no-audit".to_string(),
                "--no-fund".to_string(),
                "--prefix".to_string(),
                install_dir.to_string_lossy().to_string(),
                "pyright".to_string(),
            ],
            env: vec![],
        }),
    },
    // ── typescript-language-server ──
    ServerMeta {
        name: "typescript-language-server",
        github_repo: None,
        download_url: |_, _| String::new(),
        archive_kind: ArchiveKind::Command,
        binary_rel_path: |_| "node_modules/.bin/typescript-language-server".to_string(),
        install_command: Some(|_plat, install_dir| InstallCommand {
            program: "npm".to_string(),
            args: vec![
                "install".to_string(),
                "--no-audit".to_string(),
                "--no-fund".to_string(),
                "--prefix".to_string(),
                install_dir.to_string_lossy().to_string(),
                "typescript".to_string(),
                "typescript-language-server".to_string(),
            ],
            env: vec![],
        }),
    },
    // ── julia-lsp (LanguageServer.jl) ──
    // Installs into the shared `@simplecc` environment via Pkg, not a managed
    // binary; handled specially in do_install / installed_binary_path.
    ServerMeta {
        name: "julia-lsp",
        github_repo: None,
        download_url: |_, _| String::new(),
        archive_kind: ArchiveKind::Command,
        binary_rel_path: |_| "julia".to_string(),
        install_command: None,
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
            let installed = is_server_installed(s.name);
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
    let meta = find_server_meta(name).ok_or_else(|| anyhow::anyhow!("unknown server: {}", name))?;

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
    // Julia LSP is a package installed into a shared environment, not a binary.
    if meta.name == "julia-lsp" {
        return install_julia_lsp(event_tx).await;
    }

    let plat = current_platform();
    plat.ensure_supported()?;
    let install_dir = server_install_dir(meta.name);
    let parent = install_dir
        .parent()
        .context("managed server path has no parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .context("failed to create managed server directory")?;
    let staging_dir = unique_sibling_path(&install_dir, "installing");

    // Build the complete installation beside the active one. A failed
    // download, extraction, npm, or Go command can then be discarded without
    // corrupting a working server or making a partial binary look installed.
    tokio::fs::create_dir(&staging_dir)
        .await
        .context("failed to create installation staging directory")?;

    let result = async {
        match meta.archive_kind {
            ArchiveKind::Command => {
                install_via_command(meta, &plat, &staging_dir, event_tx).await?;
            }
            _ => {
                install_via_download(meta, &plat, &staging_dir, event_tx).await?;
            }
        }

        let expected = staging_dir.join((meta.binary_rel_path)(&plat));
        let staged_binary = if expected.exists() {
            expected
        } else {
            // Some upstream archives add an extra release directory. Keep the
            // discovered relative path stable when the staging tree is moved.
            let filename = expected
                .file_name()
                .context("managed binary path has no filename")?
                .to_string_lossy();
            find_binary_recursive(&staging_dir, &filename)
                .await
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "binary not found after installation: {}",
                        expected.display()
                    )
                })?
        };
        set_executable(&staged_binary)?;
        let relative_binary = staged_binary
            .strip_prefix(&staging_dir)
            .context("staged binary escaped its installation directory")?
            .to_path_buf();

        promote_installation(&staging_dir, &install_dir).await?;
        send_progress(event_tx, meta.name, "done", 100).await;
        Ok(install_dir.join(relative_binary))
    }
    .await;

    if result.is_err() {
        let _ = remove_path(&staging_dir).await;
    }
    result
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

    let extraction = tokio::task::spawn_blocking(move || -> Result<()> {
        match archive_kind {
            ArchiveKind::Gz => extract_gz(&tmp_file_clone, &install_dir_owned.join(&bin_name))?,
            ArchiveKind::TarGz => extract_tar_gz(&tmp_file_clone, &install_dir_owned)?,
            ArchiveKind::Zip => extract_zip(&tmp_file_clone, &install_dir_owned)?,
            ArchiveKind::Command => unreachable!(),
        }
        Ok(())
    })
    .await;

    // Always remove the downloaded archive, including on extraction failure.
    let _ = tokio::fs::remove_file(&tmp_file).await;
    extraction.context("language-server extraction task failed")??;

    // Set executable permissions
    let bin_path = install_dir.join((meta.binary_rel_path)(plat));
    if bin_path.exists() {
        set_executable(&bin_path)?;
    }

    Ok(())
}

async fn install_via_command(
    meta: &ServerMeta,
    plat: &Platform,
    install_dir: &Path,
    event_tx: &EventTx,
) -> Result<()> {
    let make_cmd = meta
        .install_command
        .ok_or_else(|| anyhow::anyhow!("no install command for {}", meta.name))?;

    let install = make_cmd(plat, install_dir);

    send_progress(
        event_tx,
        meta.name,
        &format!("running {} ...", install.program),
        0,
    )
    .await;

    // Check if the command tool exists
    if which::which(&install.program).is_err() {
        bail!(
            "'{}' not found in PATH. Please install it first.",
            install.program
        );
    }

    let mut command = tokio::process::Command::new(&install.program);
    command.args(&install.args);
    command.stdin(std::process::Stdio::null());
    command.kill_on_drop(true);
    for (k, v) in &install.env {
        command.env(k, v);
    }

    let output = tokio::time::timeout(std::time::Duration::from_secs(20 * 60), command.output())
        .await
        .with_context(|| format!("managed installer {} timed out", install.program))?
        .with_context(|| format!("failed to run managed installer {}", install.program))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} failed: {}", install.program, stderr.trim());
    }

    Ok(())
}

/// Install LanguageServer.jl into the dedicated `@simplecc` shared environment.
/// The configured command stays `julia`; we return it so the registry keeps it.
async fn install_julia_lsp(event_tx: &EventTx) -> Result<PathBuf> {
    if which::which("julia").is_err() {
        bail!("'julia' not found in PATH. Please install Julia first.");
    }

    send_progress(event_tx, "julia-lsp", "setting up @simplecc environment", 0).await;

    let script = "using Pkg; \
        Pkg.activate(\"simplecc\"; shared=true); \
        Pkg.add(\"LanguageServer\"); \
        Pkg.instantiate()";

    let mut command = tokio::process::Command::new("julia");
    command
        .args(["--startup-file=no", "--history-file=no", "-e", script])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30 * 60), command.output())
        .await
        .context("Julia language-server install timed out")?
        .context("failed to run julia")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("julia LanguageServer install failed: {}", stderr);
    }

    if !julia_lsp_env_project().exists() {
        bail!(
            "install reported success but {} was not created",
            julia_lsp_env_project().display()
        );
    }

    send_progress(event_tx, "julia-lsp", "done", 100).await;
    Ok(PathBuf::from("julia"))
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
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(30))
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
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        bail!("download failed: HTTP {}", resp.status());
    }

    let total = resp.content_length().unwrap_or(0);
    if total > MAX_DOWNLOAD_BYTES {
        bail!(
            "download is too large: {} bytes (limit: {} bytes)",
            total,
            MAX_DOWNLOAD_BYTES
        );
    }
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();

    let mut file = tokio::fs::File::create(dest).await?;
    let mut last_percent: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .context("download size overflow")?;
        if downloaded > MAX_DOWNLOAD_BYTES {
            bail!(
                "download exceeded the {} byte safety limit",
                MAX_DOWNLOAD_BYTES
            );
        }

        if let Some(percent) = downloaded.saturating_mul(100).checked_div(total) {
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

    let temporary = unique_sibling_path(dest, "extracting");
    let result = (|| -> Result<()> {
        let file = std::fs::File::open(src)?;
        let decoder = GzDecoder::new(file);
        let mut out = std::fs::File::create(&temporary)?;
        let written = std::io::copy(&mut decoder.take(MAX_EXTRACTED_BYTES + 1), &mut out)?;
        if written > MAX_EXTRACTED_BYTES {
            bail!("expanded gzip exceeds the safety limit");
        }
        out.sync_all()?;
        set_executable(&temporary)?;
        std::fs::rename(&temporary, dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn extract_tar_gz(src: &Path, dest_dir: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = std::fs::File::open(src)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut extracted = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            bail!("archive contains unsupported link or special-file entry");
        }
        extracted = extracted
            .checked_add(entry.header().size()?)
            .context("expanded archive size overflow")?;
        if extracted > MAX_EXTRACTED_BYTES {
            bail!("expanded tar archive exceeds the safety limit");
        }
        if !entry.unpack_in(dest_dir)? {
            bail!("archive entry escapes the installation directory");
        }
    }
    Ok(())
}

fn extract_zip(src: &Path, dest_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut extracted = 0_u64;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| anyhow::anyhow!("unsafe zip entry path: {}", entry.name()))?;

        // Strip top-level directory (e.g. clangd_18.1.3/ -> "")
        let Some(rel_path) = strip_top_dir(&enclosed) else {
            continue;
        };

        let out_path = dest_dir.join(&rel_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            extracted = extracted
                .checked_add(entry.size())
                .context("expanded archive size overflow")?;
            if extracted > MAX_EXTRACTED_BYTES {
                bail!("expanded zip archive exceeds the safety limit");
            }
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

/// Strip the top-level directory from an already validated archive path.
fn strip_top_dir(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    components.next()?;
    let relative: PathBuf = components.collect();
    (!relative.as_os_str().is_empty()).then_some(relative)
}

// ═════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════

fn unique_sibling_path(path: &Path, label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{label}-{}-{unique}", std::process::id()))
}

async fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    }
}

/// Replace an installation only after its staged tree is complete. The old
/// tree is restored if the final rename fails.
async fn promote_installation(staged: &Path, destination: &Path) -> Result<()> {
    let backup = unique_sibling_path(destination, "backup");
    let had_destination = tokio::fs::symlink_metadata(destination).await.is_ok();
    if had_destination {
        tokio::fs::rename(destination, &backup)
            .await
            .context("failed to stage the previous managed installation")?;
    }

    if let Err(error) = tokio::fs::rename(staged, destination).await {
        if had_destination {
            if let Err(restore_error) = tokio::fs::rename(&backup, destination).await {
                bail!(
                    "failed to activate managed installation ({error}); also failed to restore the previous installation ({restore_error})"
                );
            }
        }
        return Err(error).context("failed to activate managed installation");
    }

    if had_destination {
        let _ = remove_path(&backup).await;
    }
    Ok(())
}

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
        } else if path
            .file_name()
            .map(|f| f.to_string_lossy() == name)
            .unwrap_or(false)
        {
            set_executable(&path).ok();
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_path(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("simplecc-{label}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn npm_servers_install_into_the_managed_prefix() {
        for name in ["pyright", "typescript-language-server"] {
            let meta = find_server_meta(name).unwrap();
            let install_dir = Path::new("/tmp/simplecc-managed-test");
            let command = meta.install_command.unwrap()(&current_platform(), install_dir);
            assert_eq!(command.program, "npm");
            assert!(command.args.iter().any(|arg| arg == "--prefix"));
            assert!(command
                .args
                .iter()
                .any(|arg| arg == &install_dir.to_string_lossy()));
            assert!(!command.args.iter().any(|arg| arg == "-g"));
            assert!((meta.binary_rel_path)(&current_platform()).contains("node_modules/.bin"));
        }
    }

    #[test]
    fn zip_extraction_rejects_parent_directory_entries() {
        let root = temp_path("unsafe-zip");
        let archive_path = root.with_extension("zip");
        std::fs::create_dir_all(&root).unwrap();
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let escaped_name = format!("{}-escaped", root.file_name().unwrap().to_string_lossy());
        writer
            .start_file(
                format!("package/../../{escaped_name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"not safe").unwrap();
        writer.finish().unwrap();

        let result = extract_zip(&archive_path, &root);
        assert!(result.is_err());
        assert!(!root.parent().unwrap().join(escaped_name).exists());

        let _ = std::fs::remove_file(archive_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_extraction_strips_the_release_top_directory() {
        let root = temp_path("safe-zip");
        let archive_path = root.with_extension("zip");
        std::fs::create_dir_all(&root).unwrap();
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "clangd-release/bin/clangd",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"binary").unwrap();
        writer.finish().unwrap();

        extract_zip(&archive_path, &root).unwrap();
        assert_eq!(std::fs::read(root.join("bin/clangd")).unwrap(), b"binary");

        let _ = std::fs::remove_file(archive_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_gzip_extraction_never_leaves_a_destination_binary() {
        let root = temp_path("invalid-gzip");
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("server.gz");
        let destination = root.join("server");
        std::fs::write(&archive, b"not a gzip stream").unwrap();

        assert!(extract_gz(&archive, &destination).is_err());
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn install_detection_rejects_empty_or_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_path("usable-binary");
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("server");
        std::fs::write(&binary, []).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!is_usable_executable(&binary));

        std::fs::write(&binary, b"binary").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_usable_executable(&binary));

        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_usable_executable(&binary));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn staged_installation_replaces_complete_tree_atomically() {
        let root = temp_path("promote");
        let destination = root.join("server");
        let staged = root.join("staged");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(destination.join("version"), b"old").unwrap();
        std::fs::write(staged.join("version"), b"new").unwrap();

        promote_installation(&staged, &destination).await.unwrap();

        assert_eq!(std::fs::read(destination.join("version")).unwrap(), b"new");
        assert!(!staged.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);

        let _ = std::fs::remove_dir_all(root);
    }
}
