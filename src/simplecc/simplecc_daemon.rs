mod config;
mod installer;
mod lsp;
mod registry;
mod workspace_watcher;

use anyhow::Result;
use lsp::client::LspClient;
use lsp::types;
use registry::{EventTx, Registry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use workspace_watcher::WorkspaceWatcher;

// ─── Vim → Daemon request types ──────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum Request {
    #[serde(rename = "initialize")]
    Initialize {
        id: u64,
        root: String,
        #[serde(default)]
        config_path: Option<String>,
    },
    #[serde(rename = "shutdown")]
    Shutdown { id: u64 },

    // LanguageServer.jl extension requests
    #[serde(rename = "julia/activateEnvironment")]
    JuliaActivateEnvironment {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        #[serde(rename = "envPath")]
        env_path: String,
    },
    #[serde(rename = "julia/refreshLanguageServer")]
    JuliaRefreshLanguageServer {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
    },
    #[serde(rename = "workspace/reloadConfiguration")]
    ReloadConfiguration {
        id: u64,
        #[serde(rename = "configPath", default)]
        config_path: Option<String>,
    },

    // Document sync
    #[serde(rename = "textDocument/didOpen")]
    DidOpen {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        version: i32,
        text: String,
    },
    #[serde(rename = "textDocument/didChange")]
    DidChange {
        id: u64,
        uri: String,
        version: i32,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        changes: Option<Vec<serde_json::Value>>,
    },
    #[serde(rename = "textDocument/didSave")]
    DidSave {
        id: u64,
        uri: String,
        #[serde(default)]
        text: Option<String>,
    },
    #[serde(rename = "textDocument/didClose")]
    DidClose { id: u64, uri: String },

    // LSP features
    #[serde(rename = "textDocument/completion")]
    Completion {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
        #[serde(rename = "maxItems", default = "default_completion_max_items")]
        max_items: usize,
        #[serde(rename = "triggerKind", default = "default_completion_trigger_kind")]
        trigger_kind: u32,
        #[serde(rename = "triggerCharacter", default)]
        trigger_character: String,
    },
    #[serde(rename = "textDocument/hover")]
    Hover {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "textDocument/definition")]
    Definition {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
        #[serde(default)]
        symbol: String,
    },
    #[serde(rename = "textDocument/references")]
    References {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "textDocument/codeAction")]
    CodeAction {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
        #[serde(default)]
        end_line: Option<u32>,
        #[serde(default)]
        end_character: Option<u32>,
        #[serde(default)]
        diagnostics: Value,
    },
    #[serde(rename = "textDocument/executeAction")]
    ExecuteAction {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        index: usize,
    },
    #[serde(rename = "textDocument/formatting")]
    Formatting {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        #[serde(default = "default_tab_size")]
        tab_size: u32,
        #[serde(default = "default_true")]
        insert_spaces: bool,
    },
    #[serde(rename = "textDocument/rename")]
    Rename {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
        #[serde(rename = "newName")]
        new_name: String,
    },
    #[serde(rename = "textDocument/signatureHelp")]
    SignatureHelp {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },

    #[serde(rename = "textDocument/implementation")]
    Implementation {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "textDocument/typeDefinition")]
    TypeDefinition {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "textDocument/documentSymbol")]
    DocumentSymbol {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
    },
    #[serde(rename = "workspace/symbol")]
    WorkspaceSymbol {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        query: String,
    },
    #[serde(rename = "textDocument/documentHighlight")]
    DocumentHighlight {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "textDocument/inlayHint")]
    InlayHint {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        #[serde(rename = "startLine")]
        start_line: u32,
        #[serde(rename = "endLine")]
        end_line: u32,
    },
    #[serde(rename = "textDocument/prepareCallHierarchy")]
    PrepareCallHierarchy {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "callHierarchy/incomingCalls")]
    IncomingCalls {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        item: serde_json::Value,
    },
    #[serde(rename = "callHierarchy/outgoingCalls")]
    OutgoingCalls {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        item: serde_json::Value,
    },
    #[serde(rename = "textDocument/selectionRange")]
    SelectionRange {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        positions: Vec<serde_json::Value>,
    },
    #[serde(rename = "textDocument/semanticTokens")]
    SemanticTokensFull {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
    },
    #[serde(rename = "textDocument/semanticTokens/delta")]
    SemanticTokensDelta {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
    },
    #[serde(rename = "textDocument/semanticTokens/range")]
    SemanticTokensRange {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        #[serde(rename = "startLine")]
        start_line: u32,
        #[serde(rename = "startCharacter")]
        start_character: u32,
        #[serde(rename = "endLine")]
        end_line: u32,
        #[serde(rename = "endCharacter")]
        end_character: u32,
    },
    #[serde(rename = "textDocument/codeLens")]
    CodeLens {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
    },
    #[serde(rename = "textDocument/foldingRange")]
    FoldingRange {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
    },
    #[serde(rename = "textDocument/linkedEditingRange")]
    LinkedEditingRange {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },

    // Completion resolve
    #[serde(rename = "completionItem/resolve")]
    CompletionResolve {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        generation: u64,
        index: usize,
    },

    // Code lens execute
    #[serde(rename = "codeLens/execute")]
    ExecuteCodeLens {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        index: usize,
    },

    // Type hierarchy
    #[serde(rename = "textDocument/prepareTypeHierarchy")]
    PrepareTypeHierarchy {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
    },
    #[serde(rename = "typeHierarchy/supertypes")]
    Supertypes {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        item: serde_json::Value,
    },
    #[serde(rename = "typeHierarchy/subtypes")]
    Subtypes {
        id: u64,
        #[serde(rename = "languageId")]
        language_id: String,
        item: serde_json::Value,
    },

    // Pull diagnostics
    #[serde(rename = "textDocument/pullDiagnostics")]
    PullDiagnostics {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
    },

    // Server install
    #[serde(rename = "server/install")]
    InstallServer { id: u64, server: String },
    #[serde(rename = "server/listInstallable")]
    ListInstallable { id: u64 },
}

impl Request {
    fn preserves_document_order(&self) -> bool {
        matches!(
            self,
            Self::Initialize { .. }
                | Self::Shutdown { .. }
                | Self::DidOpen { .. }
                | Self::DidChange { .. }
                | Self::DidSave { .. }
                | Self::DidClose { .. }
                | Self::JuliaActivateEnvironment { .. }
                | Self::JuliaRefreshLanguageServer { .. }
                | Self::ReloadConfiguration { .. }
        )
    }

    fn is_lifecycle_barrier(&self) -> bool {
        matches!(self, Self::Initialize { .. } | Self::Shutdown { .. })
    }
}

fn default_tab_size() -> u32 {
    4
}
fn default_true() -> bool {
    true
}
fn default_completion_max_items() -> usize {
    100
}
fn default_completion_trigger_kind() -> u32 {
    1
}

// ─── stdout writer ───────────────────────────────────────

async fn stdout_writer(mut rx: tokio::sync::mpsc::Receiver<String>) {
    let mut out = tokio::io::stdout();
    while let Some(line) = rx.recv().await {
        if out.write_all(line.as_bytes()).await.is_err() {
            break;
        }
        if out.write_all(b"\n").await.is_err() {
            break;
        }
        let _ = out.flush().await;
    }
}

fn send_event(tx: &EventTx, event: Value) {
    let s = serde_json::to_string(&event).unwrap();
    match tx.try_send(s) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(line)) => {
            // Preserve request/reply delivery under temporary stdout
            // backpressure. The cloned sender also keeps the writer alive while
            // main drains it during shutdown.
            let tx = tx.clone();
            tokio::spawn(async move {
                if tx.send(line).await.is_err() {
                    eprintln!("[simplecc] stdout channel closed before a reply was written");
                }
            });
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            eprintln!("[simplecc] stdout channel is closed; reply could not be written");
        }
    }
}

async fn primary_client(
    registry: &Arc<RwLock<Option<Registry>>>,
    language_id: &str,
) -> Option<Arc<LspClient>> {
    let registry = registry.read().await;
    registry.as_ref()?.client_for_filetype(language_id)
}

/// Resolve the active server for a request and always finish the daemon-side
/// request when none exists. A silent `None` leaves Vim waiting forever for an
/// id that can never receive a reply (uninitialized registry, unknown
/// filetype, or a server that has just stopped).
async fn primary_client_or_error(
    registry: &Arc<RwLock<Option<Registry>>>,
    out: &EventTx,
    id: u64,
    language_id: &str,
) -> Option<Arc<LspClient>> {
    let client = primary_client(registry, language_id).await;
    if client.is_none() {
        send_event(
            out,
            json!({
                "type": "error",
                "id": id,
                "message": format!(
                    "no active language server for filetype: {language_id}"
                ),
            }),
        );
    }
    client
}

async fn filetype_clients(
    registry: &Arc<RwLock<Option<Registry>>>,
    language_id: &str,
) -> Vec<Arc<LspClient>> {
    let registry = registry.read().await;
    registry
        .as_ref()
        .map(|registry| registry.clients_for_filetype(language_id))
        .unwrap_or_default()
}

// ─── Main ────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    eprintln!("[simplecc] daemon started");

    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(4096);
    let mut stdout_task = tokio::spawn(stdout_writer(out_rx));

    let registry: Arc<RwLock<Option<Registry>>> = Arc::new(RwLock::new(None));
    let workspace_watcher: Arc<Mutex<Option<WorkspaceWatcher>>> = Arc::new(Mutex::new(None));
    // Track which filetype a URI belongs to
    let uri_ft: Arc<Mutex<std::collections::HashMap<String, String>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let mut request_tasks = tokio::task::JoinSet::new();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[simplecc] bad request: {e}");
                continue;
            }
        };

        let reg = registry.clone();
        let out = out_tx.clone();
        let uft = uri_ft.clone();
        let watcher = workspace_watcher.clone();

        // Reap completed feature tasks so a long-running daemon does not retain
        // their task records until shutdown.
        while let Some(result) = request_tasks.try_join_next() {
            if let Err(error) = result {
                eprintln!("[simplecc] request task failed: {error}");
            }
        }

        // Document notifications must reach the LSP in input order. Spawning
        // didChange and completion independently lets completion win the race
        // and query stale text.
        if req.is_lifecycle_barrier() {
            // Reinitialization and shutdown invalidate every cloned client held
            // by an in-flight feature request. Cancel those tasks before taking
            // down the registry so they cannot write after the lifecycle edge.
            request_tasks.abort_all();
            while request_tasks.join_next().await.is_some() {}
            handle_request(req, reg, out, uft, watcher).await;
        } else if req.preserves_document_order() {
            handle_request(req, reg, out, uft, watcher).await;
        } else {
            request_tasks.spawn(async move {
                handle_request(req, reg, out, uft, watcher).await;
            });
        }
    }

    // Shutdown
    request_tasks.abort_all();
    while request_tasks.join_next().await.is_some() {}
    workspace_watcher.lock().await.take();
    let mut registry_to_shutdown = registry.write().await.take();
    if let Some(ref mut reg) = registry_to_shutdown {
        reg.shutdown_all().await;
    }
    drop(registry_to_shutdown);

    // The stdout writer owns the actual pipe. Let it drain every queued reply
    // (especially the shutdown acknowledgement) before the Tokio runtime tears
    // down spawned tasks at process exit.
    drop(out_tx);
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut stdout_task)
        .await
        .is_err()
    {
        stdout_task.abort();
        let _ = stdout_task.await;
    }

    eprintln!("[simplecc] daemon exiting");
    Ok(())
}

async fn handle_request(
    req: Request,
    registry: Arc<RwLock<Option<Registry>>>,
    out: EventTx,
    uri_ft: Arc<Mutex<std::collections::HashMap<String, String>>>,
    workspace_watcher: Arc<Mutex<Option<WorkspaceWatcher>>>,
) {
    match req {
        Request::Initialize {
            id,
            root,
            config_path,
        } => {
            let cfg = match config::Config::load_selected(&root, config_path.as_deref()) {
                Ok(config) => config,
                Err(error) => {
                    send_event(
                        &out,
                        json!({
                            "type": "error",
                            "id": id,
                            "message": format!("failed to load SimpleCC configuration: {error}"),
                        }),
                    );
                    return;
                }
            };

            // A successful reinitialization replaces one complete workspace;
            // stop its watcher and servers before publishing the new registry.
            workspace_watcher.lock().await.take();
            let mut registry_to_shutdown = registry.write().await.take();
            if let Some(ref mut registry) = registry_to_shutdown {
                registry.shutdown_all().await;
            }
            uri_ft.lock().await.clear();

            let reg = Registry::new(cfg, root.clone(), out.clone());
            *registry.write().await = Some(reg);

            match WorkspaceWatcher::start(&root, registry.clone()) {
                Ok(watcher) => {
                    *workspace_watcher.lock().await = Some(watcher);
                    eprintln!("[simplecc] watching workspace: {root}");
                }
                Err(err) => {
                    eprintln!("[simplecc] workspace watcher unavailable: {err}");
                }
            }

            send_event(&out, json!({"type": "initialized", "id": id}));
        }

        Request::Shutdown { id } => {
            workspace_watcher.lock().await.take();
            let mut registry_to_shutdown = registry.write().await.take();
            if let Some(ref mut reg) = registry_to_shutdown {
                reg.shutdown_all().await;
            }
            uri_ft.lock().await.clear();
            send_event(&out, json!({"type": "shutdown", "id": id}));
        }

        Request::JuliaActivateEnvironment {
            id,
            language_id,
            env_path,
        } => {
            if language_id != "julia" {
                send_event(
                    &out,
                    json!({
                        "type": "error",
                        "id": id,
                        "message": "Julia environment activation requires a Julia buffer",
                    }),
                );
                return;
            }

            match primary_client(&registry, &language_id).await {
                Some(client) => match client.julia_activate_environment(&env_path).await {
                    Ok(()) => {
                        if let Some(watcher) = workspace_watcher.lock().await.as_mut() {
                            if let Err(err) = watcher.watch_julia_environment(&env_path) {
                                eprintln!("[simplecc] {err}");
                            }
                        }
                        send_event(
                            &out,
                            json!({
                                "type": "juliaEnvironment",
                                "id": id,
                                "path": env_path,
                            }),
                        );
                    }
                    Err(err) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": err.to_string()}),
                    ),
                },
                None => send_event(
                    &out,
                    json!({
                        "type": "error",
                        "id": id,
                        "message": "Julia language server is not running",
                    }),
                ),
            }
        }

        Request::JuliaRefreshLanguageServer { id, language_id } => {
            if language_id != "julia" {
                send_event(
                    &out,
                    json!({
                        "type": "error",
                        "id": id,
                        "message": "Julia language server refresh requires a Julia buffer",
                    }),
                );
                return;
            }

            match primary_client(&registry, &language_id).await {
                Some(client) => match client.refresh_julia_language_server().await {
                    Ok(true) => send_event(&out, json!({"type": "juliaRefreshed", "id": id})),
                    Ok(false) => send_event(
                        &out,
                        json!({
                            "type": "error",
                            "id": id,
                            "message": "Active server is not Julia LanguageServer",
                        }),
                    ),
                    Err(err) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": err.to_string()}),
                    ),
                },
                None => send_event(
                    &out,
                    json!({
                        "type": "error",
                        "id": id,
                        "message": "Julia language server is not running",
                    }),
                ),
            }
        }

        Request::ReloadConfiguration { id, config_path } => {
            let result = {
                let mut registry = registry.write().await;
                match registry.as_mut() {
                    Some(registry) => registry.reload_configuration(config_path.as_deref()).await,
                    None => Err(anyhow::anyhow!("SimpleCC is not initialized")),
                }
            };

            match result {
                Ok(server_count) => send_event(
                    &out,
                    json!({
                        "type": "configurationReloaded",
                        "id": id,
                        "servers": server_count,
                    }),
                ),
                Err(err) => send_event(
                    &out,
                    json!({"type": "error", "id": id, "message": err.to_string()}),
                ),
            }
        }

        Request::DidOpen {
            id: _,
            uri,
            language_id,
            version,
            text,
        } => {
            // Track filetype
            uri_ft.lock().await.insert(uri.clone(), language_id.clone());

            let clients = {
                let mut registry = registry.write().await;
                if let Some(ref mut registry) = *registry {
                    if let Ok(Some(_name)) = registry.ensure_server(&language_id, &uri).await {
                        registry.clients_for_filetype(&language_id)
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            };
            for client in clients {
                let _ = client.did_open(&uri, &language_id, version, &text).await;
            }
        }

        Request::DidChange {
            id: _,
            uri,
            version,
            text,
            changes,
        } => {
            let ft = uri_ft.lock().await.get(&uri).cloned();
            if let Some(ft) = ft {
                for client in filetype_clients(&registry, &ft).await {
                    let c = client;
                    let _ = c
                        .did_change(&uri, version, text.as_deref(), changes.clone())
                        .await;
                }
            }
        }

        Request::DidSave { id: _, uri, text } => {
            let ft = uri_ft.lock().await.get(&uri).cloned();
            if let Some(ft) = ft {
                for client in filetype_clients(&registry, &ft).await {
                    let c = client;
                    let _ = c.did_save(&uri, text.as_deref()).await;
                }
            }

            // A save can affect a different language client: Project.toml,
            // Manifest.toml, and closed Julia source files all invalidate the
            // Julia workspace index. Each client filters this through methods
            // it dynamically registered during initialization.
            let clients = registry
                .read()
                .await
                .as_ref()
                .map(Registry::active_clients)
                .unwrap_or_default();
            for client in clients {
                let _ = client.did_change_watched_file(&uri).await;
            }
        }

        Request::DidClose { id: _, uri } => {
            let ft = uri_ft.lock().await.remove(&uri);
            if let Some(ft) = ft {
                for client in filetype_clients(&registry, &ft).await {
                    let c = client;
                    let _ = c.did_close(&uri).await;
                }
            }
        }

        Request::Completion {
            id,
            uri,
            language_id,
            line,
            character,
            max_items,
            trigger_kind,
            trigger_character,
        } => {
            // Do not retain the global registry lock while waiting for a
            // language server. A slow completion must not block unrelated
            // servers, initialization, status, or installation requests.
            let client = primary_client(&registry, &language_id).await;

            if let Some(client) = client {
                // Clone the internally synchronized client and release the
                // outer mutex before waiting on the language server.
                let c = client;
                let trigger_character = if trigger_character.is_empty() {
                    None
                } else {
                    Some(trigger_character.as_str())
                };
                match c
                    .completion(
                        &uri,
                        line,
                        character,
                        max_items,
                        trigger_kind,
                        trigger_character,
                    )
                    .await
                {
                    Ok(Some((generation, items))) => send_event(
                        &out,
                        json!({
                            "type": "completion", "id": id,
                            "generation": generation, "items": items
                        }),
                    ),
                    Ok(None) => {}
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            } else {
                send_event(
                    &out,
                    json!({"type": "completion", "id": id, "generation": 0, "items": []}),
                );
            }
        }

        Request::Hover {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            eprintln!(
                "[simplecc] hover request: uri={} lang={} line={} char={}",
                uri, language_id, line, character
            );

            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.hover(&uri, line, character).await {
                    Ok(Some(contents)) => {
                        eprintln!("[simplecc] hover result: {} bytes", contents.len());
                        send_event(
                            &out,
                            json!({"type": "hover", "id": id, "contents": contents}),
                        );
                    }
                    Ok(None) => {
                        eprintln!("[simplecc] hover result: none");
                        send_event(&out, json!({"type": "hover", "id": id, "contents": null}));
                    }
                    Err(e) => {
                        eprintln!("[simplecc] hover error: {}", e);
                        send_event(
                            &out,
                            json!({"type": "error", "id": id, "message": e.to_string()}),
                        );
                    }
                }
            } else {
                eprintln!("[simplecc] hover: no client for filetype: {}", language_id);
            }
        }

        Request::Definition {
            id,
            uri,
            language_id,
            line,
            character,
            symbol,
        } => {
            eprintln!(
                "[simplecc] definition request: uri={} lang={} line={} char={}",
                uri, language_id, line, character
            );

            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.definition(&uri, line, character).await {
                    Ok(mut locs) => {
                        // LanguageServer.jl can fail to connect `using Package: name`
                        // references in test files to the package's live workspace
                        // source. Its workspace index still has the exact local
                        // declaration, so use that only when normal definition and
                        // document-link navigation both returned nothing.
                        if locs.is_empty() && !symbol.is_empty() {
                            match c.workspace_symbol_locations(&symbol).await {
                                Ok(fallback) => {
                                    if !fallback.is_empty() {
                                        eprintln!(
                                            "[simplecc] definition workspace fallback: symbol={} locations={}",
                                            symbol,
                                            fallback.len()
                                        );
                                        locs = fallback;
                                    }
                                }
                                Err(err) => eprintln!(
                                    "[simplecc] definition workspace fallback unavailable: {err}"
                                ),
                            }
                        }
                        eprintln!("[simplecc] definition result: {} locations", locs.len());
                        send_event(
                            &out,
                            json!({"type": "definition", "id": id, "locations": locs}),
                        );
                    }
                    Err(e) => {
                        eprintln!("[simplecc] definition error: {}", e);
                        send_event(
                            &out,
                            json!({"type": "error", "id": id, "message": e.to_string()}),
                        );
                    }
                }
            } else {
                eprintln!("[simplecc] no client for filetype: {}", language_id);
            }
        }

        Request::References {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.references(&uri, line, character).await {
                    Ok(locs) => send_event(
                        &out,
                        json!({"type": "references", "id": id, "locations": locs}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::CodeAction {
            id,
            uri,
            language_id,
            line,
            character,
            end_line,
            end_character,
            diagnostics,
        } => {
            let el = end_line.unwrap_or(line);
            let ec = end_character.unwrap_or(character);

            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c
                    .code_action(&uri, line, character, el, ec, diagnostics)
                    .await
                {
                    Ok(actions) => send_event(
                        &out,
                        json!({"type": "codeAction", "id": id, "actions": actions}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::ExecuteAction {
            id,
            language_id,
            index,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.execute_code_action(index).await {
                    Ok(Some(edit)) => {
                        send_event(&out, json!({"type": "applyEdit", "id": id, "edit": edit}))
                    }
                    Ok(None) => send_event(&out, json!({"type": "executeAction", "id": id})),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::Formatting {
            id,
            uri,
            language_id,
            tab_size,
            insert_spaces,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.formatting(&uri, tab_size, insert_spaces).await {
                    Ok(edits) => send_event(
                        &out,
                        json!({"type": "formatting", "id": id, "edits": edits}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::Rename {
            id,
            uri,
            language_id,
            line,
            character,
            new_name,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.rename(&uri, line, character, &new_name).await {
                    Ok(Some(edit)) => {
                        send_event(&out, json!({"type": "rename", "id": id, "edit": edit}))
                    }
                    Ok(None) => send_event(&out, json!({"type": "rename", "id": id, "edit": null})),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::SignatureHelp {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.signature_help(&uri, line, character).await {
                    Ok(Some(sigs)) => send_event(
                        &out,
                        json!({"type": "signatureHelp", "id": id, "signatures": sigs}),
                    ),
                    Ok(None) => send_event(
                        &out,
                        json!({"type": "signatureHelp", "id": id, "signatures": null}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::Implementation {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.implementation(&uri, line, character).await {
                    Ok(locs) => send_event(
                        &out,
                        json!({"type": "implementation", "id": id, "locations": locs}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::TypeDefinition {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.type_definition(&uri, line, character).await {
                    Ok(locs) => send_event(
                        &out,
                        json!({"type": "typeDefinition", "id": id, "locations": locs}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::DocumentSymbol {
            id,
            uri,
            language_id,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.document_symbol(&uri).await {
                    Ok(symbols) => send_event(
                        &out,
                        json!({"type": "documentSymbol", "id": id, "symbols": symbols}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::WorkspaceSymbol {
            id,
            language_id,
            query,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.workspace_symbol(&query).await {
                    Ok(symbols) => send_event(
                        &out,
                        json!({"type": "workspaceSymbol", "id": id, "symbols": symbols}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::DocumentHighlight {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.document_highlight(&uri, line, character).await {
                    Ok(highlights) => send_event(
                        &out,
                        json!({"type": "documentHighlight", "id": id, "highlights": highlights}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::InlayHint {
            id,
            uri,
            language_id,
            start_line,
            end_line,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.inlay_hints(&uri, start_line, end_line).await {
                    Ok(hints) => {
                        send_event(&out, json!({"type": "inlayHint", "id": id, "hints": hints}))
                    }
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::PrepareCallHierarchy {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.call_hierarchy_prepare(&uri, line, character).await {
                    Ok(items) => {
                        let converted: Vec<_> = items
                            .iter()
                            .map(|i| {
                                json!({
                                    "name": i.name,
                                    "kind": types::symbol_kind_label(i.kind),
                                    "uri": i.uri.to_string(),
                                    "line": i.selection_range.start.line,
                                    "character": i.selection_range.start.character,
                                    "detail": i.detail,
                                    "raw": serde_json::to_value(i).ok(),
                                })
                            })
                            .collect();
                        send_event(
                            &out,
                            json!({"type": "callHierarchyPrepare", "id": id, "items": converted}),
                        );
                    }
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::IncomingCalls {
            id,
            language_id,
            item,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                let lsp_item = match serde_json::from_value::<lsp_types::CallHierarchyItem>(item) {
                    Ok(item) => item,
                    Err(error) => {
                        send_event(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid call hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.call_hierarchy_incoming(&lsp_item).await {
                    Ok(calls) => send_event(
                        &out,
                        json!({"type": "incomingCalls", "id": id, "calls": calls}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::OutgoingCalls {
            id,
            language_id,
            item,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                let lsp_item = match serde_json::from_value::<lsp_types::CallHierarchyItem>(item) {
                    Ok(item) => item,
                    Err(error) => {
                        send_event(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid call hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.call_hierarchy_outgoing(&lsp_item).await {
                    Ok(calls) => send_event(
                        &out,
                        json!({"type": "outgoingCalls", "id": id, "calls": calls}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::SelectionRange {
            id,
            uri,
            language_id,
            positions,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                let pos: Vec<(u32, u32)> = positions
                    .iter()
                    .filter_map(|p| {
                        Some((
                            p.get("line")?.as_u64()? as u32,
                            p.get("character")?.as_u64()? as u32,
                        ))
                    })
                    .collect();
                match c.selection_range(&uri, &pos).await {
                    Ok(ranges) => send_event(
                        &out,
                        json!({"type": "selectionRange", "id": id, "ranges": ranges}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::SemanticTokensFull {
            id,
            uri,
            language_id,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.semantic_tokens_full(&uri).await {
                    Ok(tokens) => send_event(
                        &out,
                        json!({"type": "semanticTokens", "id": id, "tokens": tokens}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::SemanticTokensDelta {
            id,
            uri,
            language_id,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.semantic_tokens_full_delta(&uri).await {
                    Ok(tokens) => send_event(
                        &out,
                        json!({"type": "semanticTokens", "id": id, "tokens": tokens}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::SemanticTokensRange {
            id,
            uri,
            language_id,
            start_line,
            start_character,
            end_line,
            end_character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c
                    .semantic_tokens_range(
                        &uri,
                        start_line,
                        start_character,
                        end_line,
                        end_character,
                    )
                    .await
                {
                    Ok(tokens) => send_event(
                        &out,
                        json!({"type": "semanticTokens", "id": id, "tokens": tokens}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::CodeLens {
            id,
            uri,
            language_id,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.code_lens(&uri).await {
                    Ok(lenses) => send_event(
                        &out,
                        json!({"type": "codeLens", "id": id, "lenses": lenses}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::FoldingRange {
            id,
            uri,
            language_id,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.folding_range(&uri).await {
                    Ok(ranges) => send_event(
                        &out,
                        json!({"type": "foldingRange", "id": id, "ranges": ranges}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::LinkedEditingRange {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.linked_editing_range(&uri, line, character).await {
                    Ok(Some(ranges)) => send_event(
                        &out,
                        json!({"type": "linkedEditingRange", "id": id, "result": ranges}),
                    ),
                    Ok(None) => send_event(
                        &out,
                        json!({"type": "linkedEditingRange", "id": id, "result": null}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::CompletionResolve {
            id,
            language_id,
            generation,
            index,
        } => {
            let client = primary_client_or_error(&registry, &out, id, &language_id).await;
            if let Some(client) = client {
                let c = client;
                match c.completion_resolve(generation, index).await {
                    Ok(item) => send_event(
                        &out,
                        json!({"type": "completionResolve", "id": id, "item": item}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::ExecuteCodeLens {
            id,
            language_id,
            index,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.execute_code_lens(index).await {
                    Ok(Some(edit)) => {
                        send_event(&out, json!({"type": "applyEdit", "id": id, "edit": edit}))
                    }
                    Ok(None) => send_event(&out, json!({"type": "codeLensExecute", "id": id})),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::PrepareTypeHierarchy {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.type_hierarchy_prepare(&uri, line, character).await {
                    Ok(items) => {
                        let converted: Vec<_> = items
                            .iter()
                            .map(|i| {
                                json!({
                                    "name": i.name,
                                    "kind": types::symbol_kind_label(i.kind),
                                    "uri": i.uri.to_string(),
                                    "line": i.selection_range.start.line,
                                    "character": i.selection_range.start.character,
                                    "detail": i.detail,
                                    "raw": serde_json::to_value(i).ok(),
                                })
                            })
                            .collect();
                        send_event(
                            &out,
                            json!({"type": "typeHierarchyPrepare", "id": id, "items": converted}),
                        );
                    }
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::Supertypes {
            id,
            language_id,
            item,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                let lsp_item = match serde_json::from_value::<lsp_types::TypeHierarchyItem>(item) {
                    Ok(item) => item,
                    Err(error) => {
                        send_event(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid type hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.type_hierarchy_supertypes(&lsp_item).await {
                    Ok(items) => send_event(
                        &out,
                        json!({"type": "supertypes", "id": id, "items": items}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::Subtypes {
            id,
            language_id,
            item,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                let lsp_item = match serde_json::from_value::<lsp_types::TypeHierarchyItem>(item) {
                    Ok(item) => item,
                    Err(error) => {
                        send_event(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid type hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.type_hierarchy_subtypes(&lsp_item).await {
                    Ok(items) => {
                        send_event(&out, json!({"type": "subtypes", "id": id, "items": items}))
                    }
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::PullDiagnostics {
            id,
            uri,
            language_id,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.pull_diagnostics(&uri).await {
                    Ok(items) => send_event(
                        &out,
                        json!({"type": "diagnostics", "id": id, "uri": uri, "items": items}),
                    ),
                    Err(e) => send_event(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::InstallServer { id, server } => {
            let out_clone = out.clone();
            let reg_clone = registry.clone();
            // Spawn install in background so it doesn't block the main loop
            tokio::spawn(async move {
                match installer::install_server(&server, &out_clone).await {
                    Ok(path) => {
                        send_event(
                            &out_clone,
                            json!({
                                "type": "installResult",
                                "id": id,
                                "server": server,
                                "status": "ok",
                                "path": path.to_string_lossy(),
                            }),
                        );
                        // Update registry so the server can be started with the new path
                        let mut r = reg_clone.write().await;
                        if let Some(ref mut reg) = *r {
                            reg.update_server_command(&server, &path.to_string_lossy());
                        }
                    }
                    Err(e) => {
                        eprintln!("[simplecc] install {} failed: {}", server, e);
                        send_event(
                            &out_clone,
                            json!({
                                "type": "installResult",
                                "id": id,
                                "server": server,
                                "status": "error",
                                "message": e.to_string(),
                            }),
                        );
                    }
                }
            });
        }

        Request::ListInstallable { id } => {
            let servers = installer::list_installable();
            send_event(
                &out,
                json!({
                    "type": "installableServers",
                    "id": id,
                    "servers": servers,
                }),
            );
        }
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn parses_julia_environment_activation() {
        let request: Request = serde_json::from_value(json!({
            "type": "julia/activateEnvironment",
            "id": 7,
            "languageId": "julia",
            "envPath": "/tmp/JuliaProject"
        }))
        .unwrap();

        match request {
            Request::JuliaActivateEnvironment {
                id,
                language_id,
                env_path,
            } => {
                assert_eq!(id, 7);
                assert_eq!(language_id, "julia");
                assert_eq!(env_path, "/tmp/JuliaProject");
            }
            _ => panic!("unexpected request variant"),
        }
    }

    #[test]
    fn activation_preserves_document_order() {
        let request = Request::JuliaActivateEnvironment {
            id: 1,
            language_id: "julia".to_string(),
            env_path: "/tmp/project".to_string(),
        };
        assert!(request.preserves_document_order());
    }

    #[test]
    fn initialize_and_shutdown_are_ordered_lifecycle_barriers() {
        let initialize = Request::Initialize {
            id: 1,
            root: "/tmp/project".to_string(),
            config_path: None,
        };
        let shutdown = Request::Shutdown { id: 2 };

        assert!(initialize.preserves_document_order());
        assert!(initialize.is_lifecycle_barrier());
        assert!(shutdown.preserves_document_order());
        assert!(shutdown.is_lifecycle_barrier());
    }

    #[tokio::test]
    async fn replies_wait_for_capacity_instead_of_being_dropped() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.send("already queued".to_string()).await.unwrap();

        send_event(&tx, json!({"type": "reply", "id": 42}));

        assert_eq!(rx.recv().await.as_deref(), Some("already queued"));
        let reply = rx.recv().await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["id"], 42);
    }

    #[tokio::test]
    async fn missing_primary_client_returns_error_with_request_id() {
        let registry = Arc::new(RwLock::new(None));
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        let client = primary_client_or_error(&registry, &tx, 73, "rust").await;

        assert!(client.is_none());
        let reply: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(reply["type"], "error");
        assert_eq!(reply["id"], 73);
        assert!(reply["message"]
            .as_str()
            .unwrap()
            .contains("no active language server"));
    }

    #[test]
    fn parses_configuration_reload() {
        let request: Request = serde_json::from_value(json!({
            "type": "workspace/reloadConfiguration",
            "id": 9,
            "configPath": "/tmp/simplecc.json"
        }))
        .unwrap();

        match request {
            Request::ReloadConfiguration { id, config_path } => {
                assert_eq!(id, 9);
                assert_eq!(config_path.as_deref(), Some("/tmp/simplecc.json"));
            }
            _ => panic!("unexpected request variant"),
        }
    }

    #[test]
    fn parses_julia_language_server_refresh() {
        let request: Request = serde_json::from_value(json!({
            "type": "julia/refreshLanguageServer",
            "id": 11,
            "languageId": "julia"
        }))
        .unwrap();

        match request {
            Request::JuliaRefreshLanguageServer { id, language_id } => {
                assert_eq!(id, 11);
                assert_eq!(language_id, "julia");
            }
            _ => panic!("unexpected request variant"),
        }
    }
}
