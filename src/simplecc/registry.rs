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

        // Check if command is available
        if !is_command_available(&cfg.command) {
            eprintln!("[simplecc] command not found: {}", cfg.command);
            let event = serde_json::json!({
                "type": "serverStatus",
                "server": name,
                "status": "error",
                "message": format!("command not found: {}", cfg.command),
            });
            let _ = self.event_tx.send(serde_json::to_string(&event).unwrap()).await;
            return Ok(None);
        }

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
            &cfg.command,
            &cfg.args,
            &root_uri,
            &self.root_dir,
            cfg.initialization_options.clone(),
        ).await {
            Ok(client) => {
                let client = Arc::new(Mutex::new(client));
                self.clients.insert(name.clone(), client.clone());
                self.ft_map.insert(filetype.to_string(), name.clone());

                // Spawn event forwarder
                let server_name = name.clone();
                let event_tx2 = self.event_tx.clone();
                tokio::spawn(async move {
                    forward_server_events(client, &server_name, event_tx2).await;
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

    /// List all active servers.
    #[allow(dead_code)]
    pub fn active_servers(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }
}

/// Forward server events (diagnostics, messages) to Vim via stdout.
async fn forward_server_events(
    client: Arc<Mutex<LspClient>>,
    server_name: &str,
    event_tx: EventTx,
) {
    loop {
        let event = {
            let mut c = client.lock().await;
            c.server_events.recv().await
        };
        match event {
            Some(ServerEvent::Diagnostics { uri, diagnostics }) => {
                let ev = serde_json::json!({
                    "type": "diagnostics",
                    "uri": uri,
                    "items": diagnostics,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            Some(ServerEvent::LogMessage { level, message }) => {
                let ev = serde_json::json!({
                    "type": "log",
                    "server": server_name,
                    "level": level,
                    "message": message,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            Some(ServerEvent::ShowMessage { level, message }) => {
                let ev = serde_json::json!({
                    "type": "showMessage",
                    "server": server_name,
                    "level": level,
                    "message": message,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            Some(ServerEvent::ApplyEdit { id: _, edit }) => {
                let ev = serde_json::json!({
                    "type": "applyEdit",
                    "edit": edit,
                });
                let _ = event_tx.send(serde_json::to_string(&ev).unwrap()).await;
            }
            None => break, // Channel closed
        }
    }
}

fn is_command_available(cmd: &str) -> bool {
    which::which(cmd).is_ok()
        || std::path::Path::new(cmd).exists()
}
