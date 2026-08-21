use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::registry::EventTx;

const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_INSTALLER_STDERR_BYTES: usize = 64 * 1024;

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

struct InstallingGuard {
    name: String,
}

impl InstallingGuard {
    fn acquire(name: &str) -> Result<Self> {
        let mut set = INSTALLING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !set.insert(name.to_string()) {
            bail!("{} is already being installed", name);
        }
        Ok(Self {
            name: name.to_string(),
        })
    }
}

impl Drop for InstallingGuard {
    fn drop(&mut self) {
        INSTALLING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.name);
    }
}

/// Owns one hidden staging path without doing filesystem work in Drop.
/// Cancellation deliberately leaves `.name.installing-PID-stamp` intact for
/// startup recovery; only ordinary, fully reaped error paths call cleanup().
struct StagingGuard {
    path: PathBuf,
    finished: AtomicBool,
    cleanup_safe: AtomicBool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            finished: AtomicBool::new(false),
            cleanup_safe: AtomicBool::new(true),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn mark_promoted(&self) {
        self.finished.store(true, Ordering::Release);
    }

    fn defer_cleanup(&self) {
        self.cleanup_safe.store(false, Ordering::Release);
    }

    async fn cleanup(&self) {
        if self.finished.load(Ordering::Acquire) || !self.cleanup_safe.load(Ordering::Acquire) {
            return;
        }
        if remove_path(self.path()).await.is_ok() {
            self.finished.store(true, Ordering::Release);
        }
    }
}

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

    // Drop is cancellation-safe: lifecycle changes no longer abort installs,
    // but process shutdown or a future caller still cannot strand this
    // daemon's in-memory "already installing" marker.
    let _guard = InstallingGuard::acquire(name)?;
    do_install(meta, event_tx).await
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
    recover_stale_install_siblings_off_thread(&install_dir).await?;
    let staging_dir = unique_sibling_path(&install_dir, "installing");

    // Build the complete installation beside the active one. A failed
    // download, extraction, npm, or Go command can then be discarded without
    // corrupting a working server or making a partial binary look installed.
    tokio::fs::create_dir(&staging_dir)
        .await
        .context("failed to create installation staging directory")?;
    let staging = StagingGuard::new(staging_dir);

    let result = async {
        match meta.archive_kind {
            ArchiveKind::Command => {
                install_via_command(meta, &plat, &staging, event_tx).await?;
            }
            _ => {
                install_via_download(meta, &plat, &staging, event_tx).await?;
            }
        }

        let expected = staging.path().join((meta.binary_rel_path)(&plat));
        let staged_binary = if expected.exists() {
            expected
        } else {
            // Some upstream archives add an extra release directory. Keep the
            // discovered relative path stable when the staging tree is moved.
            let filename = expected
                .file_name()
                .context("managed binary path has no filename")?
                .to_string_lossy();
            find_binary_recursive(staging.path(), &filename)
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
            .strip_prefix(staging.path())
            .context("staged binary escaped its installation directory")?
            .to_path_buf();

        // No await inside the two-rename critical section: task abort is only
        // observed at yield points and therefore cannot strand the active
        // destination between backup and activation.
        let backup = promote_installation_sync(staging.path(), &install_dir)?;
        staging.mark_promoted();
        if let Some(backup) = backup {
            let _ = remove_path(&backup).await;
        }
        send_progress(event_tx, meta.name, "done", 100).await;
        Ok(install_dir.join(relative_binary))
    }
    .await;
    if result.is_err() {
        staging.cleanup().await;
    }
    result
}

async fn install_via_download(
    meta: &ServerMeta,
    plat: &Platform,
    staging: &StagingGuard,
    event_tx: &EventTx,
) -> Result<()> {
    let install_dir = staging.path();
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

#[cfg(unix)]
struct InstallerProcessGroup {
    pgid: Option<libc::pid_t>,
}

#[cfg(unix)]
impl InstallerProcessGroup {
    fn new(pid: Option<u32>) -> Self {
        Self {
            pgid: pid.and_then(|pid| libc::pid_t::try_from(pid).ok()),
        }
    }

    fn kill(&self) {
        if let Some(pgid) = self.pgid {
            // ESRCH simply means the whole group already exited.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
impl Drop for InstallerProcessGroup {
    fn drop(&mut self) {
        // The child is its own process-group leader. Killing the negative id
        // reaches npm/go/Julia descendants that Child::kill_on_drop cannot see.
        self.kill();
    }
}

#[cfg(not(unix))]
struct InstallerProcessGroup;

#[cfg(not(unix))]
impl InstallerProcessGroup {
    fn new(_pid: Option<u32>) -> Self {
        // Windows retains kill_on_drop for the direct child. A Job Object would
        // be needed for descendant-wide termination and is not available in
        // this dependency-light daemon.
        Self
    }

    fn kill(&self) {}
}

async fn run_installer_child(
    mut command: tokio::process::Command,
    deadline: std::time::Duration,
    description: &str,
    staging: Option<&StagingGuard>,
) -> Result<std::process::Output> {
    command
        .stdin(std::process::Stdio::null())
        // Successful installers can be extremely chatty; their stdout is not
        // actionable and must not become an unbounded in-memory Output buffer.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run {description}"))?;
    let group = InstallerProcessGroup::new(child.id());
    let stderr = child
        .stderr
        .take()
        .context("installer stderr pipe was not created")?;
    let waited = tokio::time::timeout(deadline, async {
        let (status, stderr) = tokio::join!(child.wait(), read_installer_stderr(stderr));
        Ok::<_, anyhow::Error>(std::process::Output {
            status: status.context("could not wait for installer child")?,
            stdout: Vec::new(),
            stderr: stderr.context("could not read installer stderr")?,
        })
    })
    .await;
    let output = match waited {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            if !terminate_installer_child(&group, &mut child).await
                && let Some(staging) = staging
            {
                staging.defer_cleanup();
            }
            drop(group);
            return Err(error).with_context(|| format!("failed to wait for {description}"));
        }
        Err(_) => {
            // A normal timeout is not task cancellation: explicitly stop the
            // whole group and reap the direct child before returning, so the
            // caller may safely remove staging on this ordinary Err path.
            if !terminate_installer_child(&group, &mut child).await
                && let Some(staging) = staging
            {
                staging.defer_cleanup();
            }
            drop(group);
            bail!("{description} timed out");
        }
    };
    // Also clean up any descendant that survived a normally exiting parent.
    drop(group);
    Ok(output)
}

async fn terminate_installer_child(
    group: &InstallerProcessGroup,
    child: &mut tokio::process::Child,
) -> bool {
    group.kill();
    let _ = child.start_kill();
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await,
        Ok(Ok(_))
    )
}

async fn read_installer_stderr(
    mut stderr: tokio::process::ChildStderr,
) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stderr.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = MAX_INSTALLER_STDERR_BYTES.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..read.min(remaining)]);
    }
    Ok(kept)
}

async fn install_via_command(
    meta: &ServerMeta,
    plat: &Platform,
    staging: &StagingGuard,
    event_tx: &EventTx,
) -> Result<()> {
    let install_dir = staging.path();
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
    for (k, v) in &install.env {
        command.env(k, v);
    }

    let output = run_installer_child(
        command,
        std::time::Duration::from_secs(20 * 60),
        &format!("managed installer {}", install.program),
        Some(staging),
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} failed: {}", install.program, stderr.trim());
    }

    Ok(())
}

const JULIA_INSTALL_SCRIPT: &str = "using Pkg; \
    Pkg.activate(ARGS[1]); \
    Pkg.add(\"LanguageServer\"); \
    Pkg.instantiate()";

fn julia_install_command(staging: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("julia");
    command.args([
        "--startup-file=no",
        "--history-file=no",
        "-e",
        JULIA_INSTALL_SCRIPT,
        "--",
        &staging.to_string_lossy(),
    ]);
    command
}

/// Install LanguageServer.jl into a sibling environment and atomically promote
/// it to `@simplecc`. The package depot/cache remains shared, while cancellation
/// cannot leave a half-written Project.toml as the active environment.
async fn install_julia_lsp(event_tx: &EventTx) -> Result<PathBuf> {
    if which::which("julia").is_err() {
        bail!("'julia' not found in PATH. Please install Julia first.");
    }

    send_progress(event_tx, "julia-lsp", "setting up @simplecc environment", 0).await;

    let destination = julia_lsp_env_project()
        .parent()
        .context("Julia environment marker has no parent")?
        .to_path_buf();
    let parent = destination
        .parent()
        .context("Julia environments directory has no parent")?;
    tokio::fs::create_dir_all(parent)
        .await
        .context("failed to create Julia environments directory")?;
    recover_stale_install_siblings_off_thread(&destination).await?;
    let staging_path = unique_sibling_path(&destination, "installing");
    tokio::fs::create_dir(&staging_path)
        .await
        .context("failed to create Julia installation staging directory")?;
    let staging = StagingGuard::new(staging_path);

    let result: Result<()> = async {
        let output = run_installer_child(
            julia_install_command(staging.path()),
            std::time::Duration::from_secs(30 * 60),
            "Julia language-server installer",
            Some(&staging),
        )
        .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("julia LanguageServer install failed: {}", stderr.trim());
        }

        let marker = staging.path().join("Project.toml");
        let project = std::fs::read_to_string(&marker).with_context(|| {
            format!(
                "install reported success but {} is missing",
                marker.display()
            )
        })?;
        if !project.contains("LanguageServer") {
            bail!(
                "install reported success but {} has no LanguageServer entry",
                marker.display()
            );
        }

        let backup = promote_installation_sync(staging.path(), &destination)?;
        staging.mark_promoted();
        if let Some(backup) = backup {
            let _ = remove_path(&backup).await;
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        staging.cleanup().await;
    }
    result?;

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
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            bail!("tar archive exceeds the entry-count safety limit");
        }
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
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        bail!("zip archive exceeds the entry-count safety limit");
    }
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

fn hidden_sibling_owner(path: &Path, destination: &Path, label: &str) -> Option<(u32, u128)> {
    let destination = destination.file_name()?.to_string_lossy();
    let name = path.file_name()?.to_string_lossy();
    let suffix = name.strip_prefix(&format!(".{destination}.{label}-"))?;
    let (pid, stamp) = suffix.split_once('-')?;
    Some((pid.parse().ok()?, stamp.parse().ok()?))
}

fn owner_is_stale(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        !alive && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        // Without a portable process-liveness probe, never delete another
        // process's hidden tree. Current-process leftovers are still reaped.
        false
    }
}

/// Reap cancellation leftovers and recover an old active tree if a previous
/// process died during promotion. Live foreign PIDs are never touched.
fn recover_stale_install_siblings(destination: &Path) -> Result<()> {
    let Some(parent) = destination.parent() else {
        return Ok(());
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("could not inspect installation siblings"),
    };
    let mut backups: Vec<(u128, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        for label in ["installing", "abandoned", "backup"] {
            let Some((pid, stamp)) = hidden_sibling_owner(&path, destination, label) else {
                continue;
            };
            if !owner_is_stale(pid) {
                break;
            }
            if label == "backup" {
                backups.push((stamp, path.clone()));
            } else {
                let _ = remove_path_sync(&path);
            }
            break;
        }
    }

    backups.sort_by_key(|(stamp, _)| *stamp);
    if std::fs::symlink_metadata(destination).is_err()
        && let Some((_, newest)) = backups.pop()
    {
        std::fs::rename(&newest, destination)
            .context("failed to restore interrupted managed installation")?;
    }
    for (_, backup) in backups {
        let _ = remove_path_sync(&backup);
    }
    Ok(())
}

async fn recover_stale_install_siblings_off_thread(destination: &Path) -> Result<()> {
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || recover_stale_install_siblings(&destination))
        .await
        .context("interrupted-install recovery task failed")?
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

fn remove_path_sync(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Replace an installation only after its staged tree is complete. The old
/// tree is restored if the final rename fails. Called synchronously from an
/// async task so cancellation cannot land between the two renames.
fn promote_installation_sync(staged: &Path, destination: &Path) -> Result<Option<PathBuf>> {
    let backup = unique_sibling_path(destination, "backup");
    let had_destination = std::fs::symlink_metadata(destination).is_ok();
    if had_destination {
        std::fs::rename(destination, &backup)
            .context("failed to stage the previous managed installation")?;
    }

    if let Err(error) = std::fs::rename(staged, destination) {
        if had_destination && let Err(restore_error) = std::fs::rename(&backup, destination) {
            bail!(
                "failed to activate managed installation ({error}); also failed to restore the previous installation ({restore_error})"
            );
        }
        return Err(error).context("failed to activate managed installation");
    }

    Ok(had_destination.then_some(backup))
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
    // Progress is superseded by the next progress/final result.  Never let an
    // unread stdout pipe stop the install itself before its network/command
    // deadlines can fire; the final installResult uses the daemon's reliable
    // reply path.
    let _ = event_tx.try_send(serde_json::to_string(&ev).unwrap());
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
    fn installation_marker_is_released_by_drop() {
        let name = format!("guard-test-{}", std::process::id());
        let first = InstallingGuard::acquire(&name).unwrap();
        assert!(InstallingGuard::acquire(&name).is_err());
        drop(first);
        assert!(InstallingGuard::acquire(&name).is_ok());
    }

    #[test]
    fn julia_installer_targets_the_sibling_staging_environment() {
        let staging = Path::new("/tmp/simplecc-julia-staging");
        let command = julia_install_command(staging);
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.iter().any(|arg| arg == JULIA_INSTALL_SCRIPT));
        assert!(args.iter().any(|arg| arg == staging.to_str().unwrap()));
        assert!(JULIA_INSTALL_SCRIPT.contains("Pkg.activate(ARGS[1])"));
        assert!(!JULIA_INSTALL_SCRIPT.contains("shared=true"));
    }

    #[test]
    fn stale_backup_is_restored_before_a_new_install() {
        let root = temp_path("recover-backup");
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("server");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("old"), b"old").unwrap();
        let backup = unique_sibling_path(&destination, "backup");
        std::fs::rename(&destination, &backup).unwrap();

        recover_stale_install_siblings(&destination).unwrap();
        assert_eq!(std::fs::read(destination.join("old")).unwrap(), b"old");
        assert!(!backup.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_staging_keeps_its_recoverable_installing_name() {
        let root = temp_path("cancelled-staging");
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("server");
        let staging_path = unique_sibling_path(&destination, "installing");
        std::fs::create_dir(&staging_path).unwrap();
        std::fs::write(staging_path.join("partial"), b"partial").unwrap();
        drop(StagingGuard::new(staging_path.clone()));

        assert!(staging_path.exists(), "Drop must not race an active writer");
        assert!(
            hidden_sibling_owner(&staging_path, &destination, "installing").is_some(),
            "cancellation must preserve the startup-recovery naming contract"
        );
        recover_stale_install_siblings(&destination).unwrap();
        assert!(!staging_path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn progress_is_best_effort_under_stdout_backpressure() {
        let (sender, mut rx) = tokio::sync::mpsc::channel(1);
        let tx = EventTx::new(sender);
        tx.send("already queued".to_string()).await.unwrap();
        send_progress(&tx, "rust-analyzer", "downloading", 50).await;
        assert_eq!(rx.recv().await.as_deref(), Some("already queued"));
        assert!(
            rx.try_recv().is_err(),
            "progress unexpectedly displaced a reply"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn aborting_installer_kills_its_process_group() {
        let pid_file = temp_path("installer-pids");
        let script = r#"printf '%s\n' "$$" > "$1"; sleep 30 & child=$!; printf '%s\n' "$child" >> "$1"; wait"#;
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", script, "simplecc-test", &pid_file.to_string_lossy()]);
        let runner = tokio::spawn(run_installer_child(
            command,
            std::time::Duration::from_secs(30),
            "process-group test",
            None,
        ));

        let pids = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(&pid_file).await {
                    let pids = text
                        .lines()
                        .filter_map(|line| line.parse::<libc::pid_t>().ok())
                        .collect::<Vec<_>>();
                    if pids.len() == 2 {
                        break pids;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("installer parent and child publish their PIDs");

        runner.abort();
        assert!(runner.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let all_gone = pids.iter().all(|pid| {
                    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                    match stat {
                        Err(_) => true,
                        Ok(stat) => {
                            // A zombie has exited and cannot keep writing
                            // staging; its init/subreaper may reap it later.
                            let state = stat
                                .rsplit_once(") ")
                                .and_then(|(_, rest)| rest.chars().next());
                            matches!(state, Some('Z' | 'X'))
                        }
                    }
                });
                if all_gone {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("installer process group exits after task cancellation");
        let _ = std::fs::remove_file(pid_file);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_stderr_is_drained_but_memory_bounded() {
        let script = r#"i=0; while [ "$i" -lt 10000 ]; do printf '0123456789abcdef0123456789abcdef\n' >&2; i=$((i + 1)); done; exit 7"#;
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", script]);
        let output = run_installer_child(
            command,
            std::time::Duration::from_secs(5),
            "stderr bound test",
            None,
        )
        .await
        .unwrap();
        assert!(!output.status.success());
        assert_eq!(output.stderr.len(), MAX_INSTALLER_STDERR_BYTES);
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn npm_servers_install_into_the_managed_prefix() {
        for name in ["pyright", "typescript-language-server"] {
            let meta = find_server_meta(name).unwrap();
            let install_dir = Path::new("/tmp/simplecc-managed-test");
            let command = meta.install_command.unwrap()(&current_platform(), install_dir);
            assert_eq!(command.program, "npm");
            assert!(command.args.iter().any(|arg| arg == "--prefix"));
            assert!(
                command
                    .args
                    .iter()
                    .any(|arg| arg == &install_dir.to_string_lossy())
            );
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

        let backup = promote_installation_sync(&staged, &destination)
            .unwrap()
            .expect("an old destination produces a cleanup backup");

        assert_eq!(std::fs::read(destination.join("version")).unwrap(), b"new");
        assert!(!staged.exists());
        assert!(backup.exists());
        remove_path(&backup).await.unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);

        let _ = std::fs::remove_dir_all(root);
    }
}
