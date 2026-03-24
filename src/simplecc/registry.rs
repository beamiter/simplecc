use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::config::Config;
use super::lsp::client::{LspClient, ServerEvent};

/// EventTx sends events to the stdout writer for Vim.
pub type EventTx = tokio::sync::mpsc::Sender<String>;

/// Manages multiple LSP server instances.
pub struct Registry {
    config: Config,
    /// server_name -> LspClient
    clients: HashMap<String, Arc<Mutex<LspClient>>>,
    /// filetype -> server_name
    ft_map: HashMap<String, String>,
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
    pub async fn ensure_server(&mut self, filetype: &str) -> Result<Option<String>> {
        // Already mapped?
        if let Some(name) = self.ft_map.get(filetype) {
            if self.clients.contains_key(name) {
                return Ok(Some(name.clone()));
            }
        }

        // Find server config
        let (name, cfg) = match self.config.server_for_filetype(filetype) {
            Some((n, c)) => (n.to_string(), c.clone()),
            None => return Ok(None),
        };

        // Already started?
        if self.clients.contains_key(&name) {
            self.ft_map.insert(filetype.to_string(), name.clone());
            return Ok(Some(name));
        }

        // Resolve command: PATH -> absolute path -> managed install
        let resolved_cmd = match resolve_command(&cfg.command) {
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
                let _ = self.event_tx.send(serde_json::to_string(&event).unwrap()).await;
                return Ok(None);
            }
        };

        // Start server
        let root_uri = format!("file://{}", self.root_dir);
        let event_tx = self.event_tx.clone();

        // Notify starting
        let status_event = serde_json::json!({
            "type": "serverStatus",
            "server": &name,
            "status": "starting",
        });
        let _ = event_tx.send(serde_json::to_string(&status_event).unwrap()).await;

        match LspClient::start(
            &name,
            &resolved_cmd,
            &cfg.args,
            &root_uri,
            &self.root_dir,
            cfg.initialization_options.clone(),
        ).await {
            Ok((client, event_rx)) => {
                let client = Arc::new(Mutex::new(client));
                self.clients.insert(name.clone(), client.clone());
                self.ft_map.insert(filetype.to_string(), name.clone());

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
                let _ = event_tx.send(serde_json::to_string(&status_event).unwrap()).await;

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
                let _ = event_tx.send(serde_json::to_string(&status_event).unwrap()).await;
                Ok(None)
            }
        }
    }

    /// Get the client for a filetype, if started.
    pub fn client_for_filetype(&self, filetype: &str) -> Option<Arc<Mutex<LspClient>>> {
        let name = self.ft_map.get(filetype)?;
        self.clients.get(name).cloned()
    }

    /// Get the client for a server name.
    #[allow(dead_code)]
    pub fn client_by_name(&self, name: &str) -> Option<Arc<Mutex<LspClient>>> {
        self.clients.get(name).cloned()
    }

    /// Shutdown all servers.
    pub async fn shutdown_all(&mut self) {
        for (name, client) in self.clients.drain() {
            eprintln!("[simplecc] shutting down {name}");
            let c = client.lock().await;
            let _ = c.shutdown().await;
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
        self.clients.keys().cloned().collect()
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
            ServerEvent::ApplyEdit { id: _, edit } => {
                let ev = serde_json::json!({
                    "type": "applyEdit",
                    "edit": edit,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
        }
    }
}

/// Resolve the actual command path: check managed installs first, then PATH.
/// Managed installs take priority because PATH may contain broken proxies
/// (e.g. rustup shims for components not installed in the toolchain).
fn resolve_command(cmd: &str) -> Option<String> {
    // 1. Check managed install directory first
    if let Some(path) = super::installer::installed_binary_path(cmd) {
        if path.exists() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    // 2. Check if it's an absolute path
    if std::path::Path::new(cmd).is_absolute() && std::path::Path::new(cmd).exists() {
        return Some(cmd.to_string());
    }
    // 3. Search PATH, but verify the binary is actually executable (not a broken proxy)
    if let Ok(p) = which::which(cmd) {
        if verify_executable(&p) {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}

/// Verify a binary is actually runnable (not a broken rustup shim, etc.)
fn verify_executable(path: &std::path::Path) -> bool {
    // If it's a symlink, check if the target resolves.
    // Rustup proxies are symlinks to `rustup` itself, which then fails.
    // Quick heuristic: try running with --version and check exit code.
    match std::process::Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}
