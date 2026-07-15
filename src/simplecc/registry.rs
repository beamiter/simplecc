use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::config::Config;
use super::lsp::client::{LspClient, ServerEvent};

/// EventTx sends events to the stdout writer for Vim.
pub type EventTx = tokio::sync::mpsc::Sender<String>;

/// Manages multiple LSP server instances.
pub struct Registry {
    config: Config,
    /// server_name -> LspClient
    clients: HashMap<String, Arc<LspClient>>,
    /// filetype -> list of server_names (supports multi-server per filetype)
    ft_map: HashMap<String, Vec<String>>,
    root_dir: String,
    event_tx: EventTx,
}

impl Registry {
    pub fn new(config: Config, root_dir: String, event_tx: EventTx) -> Self {
        Self {
            config,
            clients: HashMap::new(),
            ft_map: HashMap::new(),
            root_dir,
            event_tx,
        }
    }

    /// Ensure the server for a given filetype is started. Returns server name if available.
    pub async fn ensure_server(
        &mut self,
        filetype: &str,
        document_uri: &str,
    ) -> Result<Option<String>> {
        self.prune_dead_clients();

        // Already mapped and running?
        if let Some(names) = self.ft_map.get(filetype) {
            if let Some(name) = names.first() {
                if self.clients.contains_key(name) {
                    return Ok(Some(name.clone()));
                }
            }
        }

        // Find server config
        let (name, cfg) = match self.config.server_for_filetype(filetype) {
            Some((n, c)) => (n.to_string(), c.clone()),
            None => return Ok(None),
        };

        // Already started?
        if self.clients.contains_key(&name) {
            let names = self.ft_map.entry(filetype.to_string()).or_default();
            if !names.contains(&name) {
                names.push(name.clone());
            }
            return Ok(Some(name));
        }

        // Resolve command: PATH -> absolute path -> managed install
        let resolved_cmd = match resolve_command(&name, &cfg.command).await {
            Some(cmd) => cmd,
            None => {
                eprintln!("[simplecc] command not found: {}", cfg.command);
                let installable = super::installer::is_known_server(&name);
                let event = serde_json::json!({
                    "type": "serverStatus",
                    "server": name,
                    "status": "notFound",
                    "message": format!("command not found: {}", cfg.command),
                    "installable": installable,
                });
                let _ = self
                    .event_tx
                    .send(serde_json::to_string(&event).unwrap())
                    .await;
                return Ok(None);
            }
        };

        // Server-specific readiness check: julia-lsp needs LanguageServer.jl in
        // the dedicated @simplecc environment. The `julia` binary resolves fine,
        // so without this check we'd spawn a process that immediately dies.
        if name == "julia-lsp" && !super::installer::is_julia_lsp_installed() {
            eprintln!("[simplecc] julia-lsp: LanguageServer.jl not found in @simplecc environment");
            let event = serde_json::json!({
                "type": "serverStatus",
                "server": name,
                "status": "notFound",
                "message": "LanguageServer.jl is not installed in the @simplecc environment",
                "installable": true,
            });
            let _ = self
                .event_tx
                .send(serde_json::to_string(&event).unwrap())
                .await;
            return Ok(None);
        }

        // Start server
        let server_root = server_root_path(&self.root_dir, document_uri, &cfg.root_patterns);
        let root_uri = directory_uri(&server_root)?;
        let root_path = server_root.to_string_lossy().into_owned();
        let event_tx = self.event_tx.clone();

        // Notify starting
        let status_event = serde_json::json!({
            "type": "serverStatus",
            "server": &name,
            "status": "starting",
        });
        let _ = event_tx
            .send(serde_json::to_string(&status_event).unwrap())
            .await;

        match LspClient::start(
            &name,
            &resolved_cmd,
            &cfg.args,
            &root_uri,
            &root_path,
            cfg.effective_initialization_options(&name),
            cfg.effective_settings(&name),
        )
        .await
        {
            Ok((client, event_rx)) => {
                let client = Arc::new(client);
                self.clients.insert(name.clone(), client.clone());
                let names = self.ft_map.entry(filetype.to_string()).or_default();
                if !names.contains(&name) {
                    names.push(name.clone());
                }

                // Spawn event forwarder with the receiver directly (no client lock needed)
                let server_name = name.clone();
                let event_tx2 = self.event_tx.clone();
                tokio::spawn(async move {
                    forward_server_events(event_rx, &server_name, event_tx2).await;
                });

                // Notify running
                let status_event = serde_json::json!({
                    "type": "serverStatus",
                    "server": &name,
                    "status": "running",
                });
                let _ = event_tx
                    .send(serde_json::to_string(&status_event).unwrap())
                    .await;

                Ok(Some(name))
            }
            Err(e) => {
                eprintln!("[simplecc] failed to start {name}: {e}");
                let status_event = serde_json::json!({
                    "type": "serverStatus",
                    "server": &name,
                    "status": "error",
                    "message": e.to_string(),
                });
                let _ = event_tx
                    .send(serde_json::to_string(&status_event).unwrap())
                    .await;
                Ok(None)
            }
        }
    }

    /// Get the primary client for a filetype, if started.
    pub fn client_for_filetype(&self, filetype: &str) -> Option<Arc<LspClient>> {
        let names = self.ft_map.get(filetype)?;
        let name = names.first()?;
        self.clients
            .get(name)
            .filter(|client| client.is_alive())
            .cloned()
    }

    /// Get all clients for a filetype (for multi-server support).
    pub fn clients_for_filetype(&self, filetype: &str) -> Vec<Arc<LspClient>> {
        if let Some(names) = self.ft_map.get(filetype) {
            names
                .iter()
                .filter_map(|name| self.clients.get(name))
                .filter(|client| client.is_alive())
                .cloned()
                .collect()
        } else {
            vec![]
        }
    }

    /// All running clients. Used for notifications (such as watched-file
    /// changes) whose routing is determined by dynamic server registration
    /// rather than the saved buffer's filetype.
    pub fn active_clients(&self) -> Vec<Arc<LspClient>> {
        self.clients
            .values()
            .filter(|client| client.is_alive())
            .cloned()
            .collect()
    }

    /// Reload configuration and update every running server in place. New
    /// server instances also use the replacement config through `self.config`.
    pub async fn reload_configuration(&mut self, config_path: Option<&str>) -> Result<usize> {
        self.prune_dead_clients();
        let config = Config::load_selected(&self.root_dir, config_path)?;
        let updates: Vec<_> = self
            .clients
            .iter()
            .map(|(name, client)| {
                (
                    client.clone(),
                    config
                        .language_servers
                        .get(name)
                        .and_then(|server| server.effective_settings(name)),
                )
            })
            .collect();

        self.config = config;
        for (client, settings) in &updates {
            client.did_change_configuration(settings.clone()).await?;
        }
        Ok(updates.len())
    }

    /// Get the client for a server name.
    #[allow(dead_code)]
    pub fn client_by_name(&self, name: &str) -> Option<Arc<LspClient>> {
        self.clients
            .get(name)
            .filter(|client| client.is_alive())
            .cloned()
    }

    /// Shutdown all servers.
    pub async fn shutdown_all(&mut self) {
        for (name, client) in self.clients.drain() {
            eprintln!("[simplecc] shutting down {name}");
            let _ = client.shutdown().await;
        }
    }

    /// Update the command path for a server (after installation).
    pub fn update_server_command(&mut self, name: &str, command: &str) {
        if let Some(cfg) = self.config.language_servers.get_mut(name) {
            cfg.command = command.to_string();
        }
    }

    /// List all active servers.
    #[allow(dead_code)]
    pub fn active_servers(&self) -> Vec<String> {
        self.clients
            .iter()
            .filter(|(_, client)| client.is_alive())
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn prune_dead_clients(&mut self) {
        self.clients.retain(|_, client| client.is_alive());
        let live_names: HashSet<_> = self.clients.keys().cloned().collect();
        self.ft_map.retain(|_, names| {
            names.retain(|name| live_names.contains(name));
            !names.is_empty()
        });
    }
}

/// Forward server events (diagnostics, messages) to Vim via stdout.
/// Takes the receiver directly — no client lock needed, eliminating deadlocks.
async fn forward_server_events(
    mut event_rx: tokio::sync::mpsc::Receiver<ServerEvent>,
    server_name: &str,
    event_tx: EventTx,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            ServerEvent::Diagnostics { uri, diagnostics } => {
                let ev = serde_json::json!({
                    "type": "diagnostics",
                    "uri": uri,
                    "items": diagnostics,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            ServerEvent::LogMessage { level, message } => {
                let ev = serde_json::json!({
                    "type": "log",
                    "server": server_name,
                    "level": level,
                    "message": message,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            ServerEvent::ShowMessage { level, message } => {
                let ev = serde_json::json!({
                    "type": "showMessage",
                    "server": server_name,
                    "level": level,
                    "message": message,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            ServerEvent::Progress {
                token,
                kind,
                title,
                message,
                percentage,
            } => {
                let ev = serde_json::json!({
                    "type": "progress",
                    "server": server_name,
                    "token": token,
                    "kind": kind,
                    "title": title,
                    "message": message,
                    "percentage": percentage,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            ServerEvent::ApplyEdit { id: _, edit } => {
                let ev = serde_json::json!({
                    "type": "applyEdit",
                    "edit": edit,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            ServerEvent::Stopped { message } => {
                let ev = serde_json::json!({
                    "type": "serverStatus",
                    "server": server_name,
                    "status": "stopped",
                    "message": message,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
        }
    }
}

fn absolute_workspace_path(root: &str) -> PathBuf {
    let root = PathBuf::from(root);
    std::fs::canonicalize(&root).unwrap_or_else(|_| {
        if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join(root)
        }
    })
}

/// Find the nearest configured project marker for the first document that
/// starts a server. When the document belongs to the daemon workspace, never
/// walk above that workspace boundary.
fn server_root_path(root: &str, document_uri: &str, root_patterns: &[String]) -> PathBuf {
    let workspace_root = absolute_workspace_path(root);
    if root_patterns.is_empty() {
        return workspace_root;
    }

    let Some(document_path) = url::Url::parse(document_uri)
        .ok()
        .and_then(|uri| uri.to_file_path().ok())
    else {
        return workspace_root;
    };
    let Some(mut directory) = document_path.parent().map(Path::to_path_buf) else {
        return workspace_root;
    };
    if let Ok(canonical) = std::fs::canonicalize(&directory) {
        directory = canonical;
    }

    let boundary = directory
        .starts_with(&workspace_root)
        .then_some(workspace_root.as_path());
    loop {
        if root_patterns
            .iter()
            .any(|pattern| directory.join(pattern).exists())
        {
            return directory;
        }
        if boundary == Some(directory.as_path()) || !directory.pop() {
            break;
        }
    }
    workspace_root
}

fn directory_uri(path: &Path) -> Result<String> {
    url::Url::from_directory_path(path)
        .map(|uri| uri.to_string())
        .map_err(|()| {
            anyhow::anyhow!(
                "workspace root is not an absolute file path: {}",
                path.display()
            )
        })
}

/// Resolve the actual command path: check managed installs first, then PATH.
/// Managed installs take priority because PATH may contain broken proxies
/// (e.g. rustup shims for components not installed in the toolchain).
async fn resolve_command(server_name: &str, cmd: &str) -> Option<String> {
    // 1. Check managed install directory first
    // Julia's managed marker is a Project.toml rather than an executable.
    if server_name != "julia-lsp" && super::installer::is_server_installed(server_name) {
        if let Some(path) = super::installer::installed_binary_path(server_name) {
            return Some(path.to_string_lossy().to_string());
        }
    }
    // 2. Check if it's an absolute path
    if std::path::Path::new(cmd).is_absolute() && std::path::Path::new(cmd).exists() {
        return Some(cmd.to_string());
    }
    // 3. Search PATH, but verify the binary is actually executable (not a broken proxy)
    if let Ok(p) = which::which(cmd) {
        // pyright-langserver intentionally has no `--version` mode; without a
        // transport flag it exits non-zero even when the installation is valid.
        if server_name == "pyright" || verify_executable(&p).await {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}

/// Verify a binary is actually runnable (not a broken rustup shim, etc.)
async fn verify_executable(path: &Path) -> bool {
    // If it's a symlink, check if the target resolves.
    // Rustup proxies are symlinks to `rustup` itself, which then fails.
    // Quick heuristic: try running with --version and check exit code.
    let mut command = tokio::process::Command::new(path);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            let _ = child.kill().await;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_uri_percent_encodes_reserved_path_characters() {
        let uri = directory_uri(Path::new("/tmp/Simple CC#workspace")).unwrap();
        assert_eq!(uri, "file:///tmp/Simple%20CC%23workspace/");
    }

    #[test]
    fn root_patterns_choose_the_nearest_marker_without_crossing_workspace() {
        let unique = format!(
            "simplecc-registry-root-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        );
        let workspace = std::env::temp_dir().join(unique);
        let package = workspace.join("packages/app");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(package.join("Cargo.toml"), "[package]\nname='app'\n").unwrap();
        let document_uri = url::Url::from_file_path(package.join("src/main.rs"))
            .unwrap()
            .to_string();

        let selected = server_root_path(
            workspace.to_str().unwrap(),
            &document_uri,
            &["Cargo.toml".to_string()],
        );

        assert_eq!(selected, package);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn root_patterns_do_not_select_a_marker_above_the_workspace() {
        let unique = format!(
            "simplecc-registry-boundary-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        );
        let sandbox = std::env::temp_dir().join(unique);
        let workspace = sandbox.join("workspace");
        let source = workspace.join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(sandbox.join("Cargo.toml"), "[workspace]\n").unwrap();
        let document_uri = url::Url::from_file_path(source.join("main.rs"))
            .unwrap()
            .to_string();

        let selected = server_root_path(
            workspace.to_str().unwrap(),
            &document_uri,
            &["Cargo.toml".to_string()],
        );

        assert_eq!(selected, std::fs::canonicalize(&workspace).unwrap());
        std::fs::remove_dir_all(sandbox).unwrap();
    }
}
