use anyhow::{bail, Result};
use lsp_types::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use super::transport::LspTransport;
use super::types;

#[derive(Default)]
struct CompletionCache {
    generation: u64,
    items: Vec<lsp_types::CompletionItem>,
}

/// A single LSP server client.
///
/// The client is cheaply cloneable: all mutable protocol state is already
/// internally synchronized. The daemon can therefore release its outer client
/// mutex before awaiting a slow language-server request.
#[derive(Clone)]
pub struct LspClient {
    transport: Arc<Mutex<LspTransport>>,
    next_id: Arc<AtomicI64>,
    /// Pending requests: jsonrpc id -> oneshot sender
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    /// Newest in-flight request id per latest-wins feature key.
    latest_requests: Arc<Mutex<HashMap<String, i64>>>,
    /// Server capabilities after initialize
    pub capabilities: Arc<Mutex<Option<ServerCapabilities>>>,
    /// Cached code actions for execute
    pub cached_actions: Arc<Mutex<Vec<lsp_types::CodeAction>>>,
    /// Latest completion generation and its raw items for resolve.
    cached_completions: Arc<Mutex<CompletionCache>>,
    completion_generation: Arc<AtomicU64>,
    /// Cached code lenses for execute
    pub cached_code_lenses: Arc<Mutex<Vec<lsp_types::CodeLens>>>,
    /// Previous semantic token result_id per URI (for delta requests)
    semtok_prev_result_id: Arc<Mutex<HashMap<String, String>>>,
    /// Previous raw semantic token data per URI (for applying delta edits)
    semtok_prev_data: Arc<Mutex<HashMap<String, Vec<lsp_types::SemanticToken>>>>,
    /// Methods dynamically registered by the server, such as workspace file
    /// watching requested by LanguageServer.jl.
    registered_methods: Arc<Mutex<HashSet<String>>>,
    /// Settings served through `workspace/configuration`. Kept mutable so a
    /// config reload can update a running language server.
    settings: Arc<RwLock<Value>>,
    /// Recent watched-file notifications, used to collapse the duplicate
    /// event commonly produced by Vim's save hook and the platform watcher.
    watched_file_notifications: Arc<Mutex<HashMap<(String, u32), Instant>>>,
    server_name: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ServerEvent {
    Diagnostics {
        uri: String,
        diagnostics: Vec<types::DiagnosticItem>,
    },
    LogMessage {
        level: String,
        message: String,
    },
    ShowMessage {
        level: String,
        message: String,
    },
    ApplyEdit {
        id: Value,
        edit: types::WorkspaceEdit,
    },
    Progress {
        token: String,
        kind: String,
        title: String,
        message: String,
        percentage: Option<u64>,
    },
}

impl LspClient {
    /// Start a new LSP server and perform the initialize handshake.
    /// Returns (client, server_events_receiver) — the receiver is separate
    /// to avoid holding the client lock while waiting for server events.
    pub async fn start(
        server_name: &str,
        cmd: &str,
        args: &[String],
        root_uri: &str,
        root_path: &str,
        init_options: Option<Value>,
        settings: Option<Value>,
    ) -> Result<(Self, mpsc::Receiver<ServerEvent>)> {
        let (transport, mut incoming) = LspTransport::spawn(cmd, args, Some(root_path))?;

        let transport = Arc::new(Mutex::new(transport));
        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let capabilities: Arc<Mutex<Option<ServerCapabilities>>> = Arc::new(Mutex::new(None));

        let (event_tx, event_rx) = mpsc::channel::<ServerEvent>(256);

        // Spawn message dispatcher
        let pending_clone = pending.clone();
        let transport_clone = transport.clone();
        let event_tx_clone = event_tx.clone();
        let settings = Arc::new(RwLock::new(settings.unwrap_or_else(|| json!({}))));
        let settings_clone = settings.clone();
        let registered_methods = Arc::new(Mutex::new(HashSet::new()));
        let registered_methods_clone = registered_methods.clone();
        tokio::spawn(async move {
            while let Some(msg) = incoming.recv().await {
                // Is it a response?
                if let Some(id) = msg.get("id") {
                    if msg.get("method").is_some() {
                        // Server request (e.g. workspace/applyEdit)
                        handle_server_request(
                            &msg,
                            &transport_clone,
                            &event_tx_clone,
                            &settings_clone,
                            &registered_methods_clone,
                        )
                        .await;
                    } else {
                        // Response to our request
                        let id = id.as_i64().unwrap_or(0);
                        let mut map = pending_clone.lock().await;
                        if let Some(tx) = map.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                } else if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                    // Server notification
                    handle_server_notification(method, &msg, &event_tx_clone).await;
                }
            }
        });

        let mut client = Self {
            transport,
            next_id: Arc::new(AtomicI64::new(1)),
            pending,
            latest_requests: Arc::new(Mutex::new(HashMap::new())),
            capabilities,
            cached_actions: Arc::new(Mutex::new(Vec::new())),
            cached_completions: Arc::new(Mutex::new(CompletionCache::default())),
            completion_generation: Arc::new(AtomicU64::new(0)),
            cached_code_lenses: Arc::new(Mutex::new(Vec::new())),
            semtok_prev_result_id: Arc::new(Mutex::new(HashMap::new())),
            semtok_prev_data: Arc::new(Mutex::new(HashMap::new())),
            registered_methods,
            settings,
            watched_file_notifications: Arc::new(Mutex::new(HashMap::new())),
            server_name: server_name.to_string(),
        };

        // Initialize
        client.initialize(root_uri, root_path, init_options).await?;

        Ok((client, event_rx))
    }

    fn next_request_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a JSON-RPC request and wait for response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, Duration::from_secs(120))
            .await
    }

    /// Send a JSON-RPC request with a feature-specific timeout. Timed-out
    /// requests are removed from the pending map and cancelled at the server,
    /// preventing leaked senders and very late responses from accumulating.
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        match self
            .request_with_timeout_inner(None, method, params, timeout)
            .await?
        {
            Some(result) => Ok(result),
            None => bail!("non-superseding request was unexpectedly cancelled: {method}"),
        }
    }

    /// Send a request where only the newest request for `key` is useful.
    /// Starting a replacement drops the previous response channel and emits
    /// `$/cancelRequest`. Superseded calls return `Ok(None)` without producing
    /// a daemon error event.
    async fn request_latest_with_timeout(
        &self,
        key: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Option<Value>> {
        self.request_with_timeout_inner(Some(key), method, params, timeout)
            .await
    }

    async fn send_message(&self, msg: &Value) -> Result<()> {
        let mut transport = self.transport.lock().await;
        transport.send(msg).await
    }

    async fn cancel_pending_request(&self, id: i64) {
        let removed = self.pending.lock().await.remove(&id).is_some();
        if !removed {
            return;
        }

        let cancel = json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": id },
        });
        let _ = self.send_message(&cancel).await;
    }

    /// Remove `key` only if it still points at `id`. A false return means a
    /// newer request replaced this one while it was waiting for the server.
    async fn clear_latest_request(&self, key: &str, id: i64) -> bool {
        let mut latest = self.latest_requests.lock().await;
        if latest.get(key).copied() != Some(id) {
            return false;
        }
        latest.remove(key);
        true
    }

    async fn request_with_timeout_inner(
        &self,
        latest_key: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Option<Value>> {
        let id = self.next_request_id();
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        // Hold the latest-request map while cancelling the previous request and
        // writing the replacement. This guarantees the server observes
        // request(old) -> cancel(old) -> request(new), never cancel-before-send.
        let send_result = if let Some(key) = latest_key {
            let mut latest = self.latest_requests.lock().await;
            if let Some(previous_id) = latest.insert(key.to_string(), id) {
                self.cancel_pending_request(previous_id).await;
            }
            let result = self.send_message(&msg).await;
            if result.is_err() && latest.get(key).copied() == Some(id) {
                latest.remove(key);
            }
            result
        } else {
            self.send_message(&msg).await
        };

        if let Err(err) = send_result {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }

        let resp = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(err)) => {
                self.pending.lock().await.remove(&id);
                if let Some(key) = latest_key {
                    if !self.clear_latest_request(key, id).await {
                        return Ok(None);
                    }
                }
                return Err(err.into());
            }
            Err(_) => {
                self.cancel_pending_request(id).await;
                if let Some(key) = latest_key {
                    self.clear_latest_request(key, id).await;
                }
                bail!(
                    "LSP request timed out after {}ms: {}",
                    timeout.as_millis(),
                    method,
                );
            }
        };

        if let Some(key) = latest_key {
            if !self.clear_latest_request(key, id).await {
                return Ok(None);
            }
        }

        if let Some(err) = resp.get("error") {
            bail!("LSP error: {}", err);
        }

        Ok(Some(resp.get("result").cloned().unwrap_or(Value::Null)))
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_message(&msg).await
    }

    // ─── LSP Lifecycle ──────────────────────────────────────

    async fn initialize(
        &mut self,
        root_uri: &str,
        root_path: &str,
        init_options: Option<Value>,
    ) -> Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {
                "name": "SimpleCC",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "rootUri": root_uri,
            "rootPath": root_path,
            "capabilities": client_capabilities(),
            "initializationOptions": init_options.unwrap_or(Value::Null),
            "workspaceFolders": [{
                "uri": root_uri,
                "name": std::path::Path::new(root_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
            }],
        });

        let result = self.request("initialize", params).await?;

        if let Ok(init_result) = serde_json::from_value::<InitializeResult>(result) {
            *self.capabilities.lock().await = Some(init_result.capabilities);
        }

        self.notify("initialized", json!({})).await?;

        eprintln!("[simplecc] {} initialized", self.server_name);
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.request("shutdown", Value::Null).await;
        self.notify("exit", Value::Null).await?;
        Ok(())
    }

    // ─── Document Sync ──────────────────────────────────────

    /// Switch LanguageServer.jl to another Julia project environment.
    /// LanguageServer.jl reloads its project files and symbol store after the
    /// same custom notification used by the Julia VS Code extension.
    pub async fn julia_activate_environment(&self, env_path: &str) -> Result<()> {
        if self.server_name != "julia-lsp" {
            bail!("server {} is not Julia LanguageServer", self.server_name);
        }
        self.notify("julia/activateenvironment", json!({ "envPath": env_path }))
            .await
    }

    /// Ask LanguageServer.jl to rebuild its dependency symbol cache. Returns
    /// false for non-Julia clients so workspace watchers can broadcast safely.
    pub async fn refresh_julia_language_server(&self) -> Result<bool> {
        if self.server_name != "julia-lsp" {
            return Ok(false);
        }
        self.notify("julia/refreshLanguageServer", Value::Null)
            .await?;
        Ok(true)
    }

    /// Replace values returned by `workspace/configuration`, then notify the
    /// server so it can request and apply the new values.
    pub async fn did_change_configuration(&self, settings: Option<Value>) -> Result<()> {
        let settings = settings.unwrap_or_else(|| json!({}));
        *self.settings.write().await = settings.clone();
        self.notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": settings }),
        )
        .await
    }

    pub async fn did_open(
        &self,
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        )
        .await
    }

    pub async fn did_change(
        &self,
        uri: &str,
        version: i32,
        text: Option<&str>,
        changes: Option<Vec<Value>>,
    ) -> Result<()> {
        let content_changes = if let Some(changes) = changes {
            // Check if server supports incremental sync
            let caps = self.capabilities.lock().await;
            let supports_incremental = caps
                .as_ref()
                .and_then(|c| match &c.text_document_sync {
                    Some(TextDocumentSyncCapability::Kind(kind)) => {
                        Some(*kind == TextDocumentSyncKind::INCREMENTAL)
                    }
                    Some(TextDocumentSyncCapability::Options(opts)) => {
                        opts.change.map(|k| k == TextDocumentSyncKind::INCREMENTAL)
                    }
                    None => None,
                })
                .unwrap_or(false);
            drop(caps);
            if supports_incremental {
                json!(changes)
            } else if let Some(text) = text {
                json!([{ "text": text }])
            } else {
                json!([{ "text": "" }])
            }
        } else if let Some(text) = text {
            json!([{ "text": text }])
        } else {
            json!([{ "text": "" }])
        };

        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": content_changes
            }),
        )
        .await
    }

    pub async fn did_save(&self, uri: &str, text: Option<&str>) -> Result<()> {
        let mut params = json!({ "textDocument": { "uri": uri } });
        if let Some(t) = text {
            params["text"] = json!(t);
        }
        self.notify("textDocument/didSave", params).await
    }

    pub async fn did_change_watched_file(&self, uri: &str) -> Result<()> {
        self.did_change_watched_files(&[(uri.to_string(), 2)]).await
    }

    pub async fn did_change_watched_files(&self, changes: &[(String, u32)]) -> Result<()> {
        if !self
            .registered_methods
            .lock()
            .await
            .contains("workspace/didChangeWatchedFiles")
        {
            return Ok(());
        }

        let now = Instant::now();
        let mut recent = self.watched_file_notifications.lock().await;
        recent.retain(|_, seen| now.duration_since(*seen) < Duration::from_secs(2));
        let changes: Vec<_> = changes
            .iter()
            .filter_map(|(uri, change_type)| {
                let key = (uri.clone(), *change_type);
                if recent
                    .get(&key)
                    .is_some_and(|seen| now.duration_since(*seen) < Duration::from_millis(250))
                {
                    return None;
                }
                recent.insert(key, now);
                Some(json!({ "uri": uri, "type": change_type }))
            })
            .collect();
        drop(recent);
        if changes.is_empty() {
            return Ok(());
        }

        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": changes }),
        )
        .await
    }

    pub async fn did_close(&self, uri: &str) -> Result<()> {
        self.notify(
            "textDocument/didClose",
            json!({
                "textDocument": { "uri": uri }
            }),
        )
        .await
    }

    // ─── LSP Features ───────────────────────────────────────

    pub async fn completion(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        max_items: usize,
        trigger_kind: u32,
        trigger_character: Option<&str>,
    ) -> Result<Option<(u64, Vec<types::CompletionItem>)>> {
        // Allocate at request start, not response time. If an older request
        // returns after a newer one, it must never replace the newer cache.
        let generation = self.completion_generation.fetch_add(1, Ordering::SeqCst) + 1;

        // Vim can cheaply infer punctuation-triggered requests, but only the
        // server knows which trigger characters it advertised. Downgrade an
        // unsupported trigger-character request to TriggerForIncompleteCompletions
        // so servers never receive an invalid CompletionContext.
        let mut effective_trigger_kind = trigger_kind.clamp(1, 3);
        let mut effective_trigger_character = trigger_character
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if effective_trigger_kind == 2 {
            let supported = match effective_trigger_character.as_deref() {
                Some(trigger) => {
                    let capabilities = self.capabilities.lock().await;
                    capabilities
                        .as_ref()
                        .and_then(|caps| caps.completion_provider.as_ref())
                        .and_then(|options| options.trigger_characters.as_ref())
                        .map(|characters| characters.iter().any(|value| value == trigger))
                        .unwrap_or(false)
                }
                None => false,
            };
            if !supported {
                effective_trigger_kind = 3;
                effective_trigger_character = None;
            }
        } else {
            effective_trigger_character = None;
        }

        let mut context = json!({
            "triggerKind": effective_trigger_kind,
        });
        if let Some(trigger) = effective_trigger_character {
            context["triggerCharacter"] = json!(trigger);
        }

        let request_key = format!("completion:{uri}");
        let result = match self
            .request_latest_with_timeout(
                &request_key,
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": context,
                }),
                Duration::from_secs(3),
            )
            .await?
        {
            Some(result) => result,
            None => return Ok(None),
        };

        let mut items = if result.is_array() {
            serde_json::from_value::<Vec<lsp_types::CompletionItem>>(result)?
        } else if let Ok(list) = serde_json::from_value::<CompletionList>(result.clone()) {
            list.items
        } else {
            vec![]
        };
        items.truncate(max_items.max(1).min(500));

        {
            let mut cache = self.cached_completions.lock().await;
            if generation >= cache.generation {
                cache.generation = generation;
                cache.items = items.clone();
            }
        }

        let normalized = items
            .iter()
            .enumerate()
            .map(|(idx, item)| types::from_lsp_completion_item(item, idx))
            .collect();

        Ok(Some((generation, normalized)))
    }

    pub async fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<String>> {
        let result = self
            .request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(None);
        }

        let hover: Hover = serde_json::from_value(result)?;
        let text = match hover.contents {
            HoverContents::Scalar(mc) => match mc {
                MarkedString::String(s) => s,
                MarkedString::LanguageString(ls) => {
                    format!("```{}\n{}\n```", ls.language, ls.value)
                }
            },
            HoverContents::Array(arr) => arr
                .into_iter()
                .map(|mc| match mc {
                    MarkedString::String(s) => s,
                    MarkedString::LanguageString(ls) => {
                        format!("```{}\n{}\n```", ls.language, ls.value)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            HoverContents::Markup(mc) => mc.value,
        };
        Ok(Some(text))
    }

    pub async fn definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<types::Location>> {
        let result = self
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;

        let mut locations = parse_locations(result)?;

        // Some language servers expose navigable file references as document
        // links instead of definitions. LanguageServer.jl does this for paths
        // in `include("file.jl")`, so a definition-only client otherwise reports
        // "No definition found" even though the server knows the target file.
        // Keep this as a fallback so normal symbol definitions always win.
        if locations.is_empty() {
            match self.document_link_at(uri, line, character).await {
                Ok(Some(location)) => locations.push(location),
                Ok(None) => {}
                // Not every server implements document links. An unsupported
                // fallback must preserve the original empty definition result.
                Err(err) => {
                    eprintln!("[simplecc] document-link definition fallback unavailable: {err}")
                }
            }
        }

        Ok(locations)
    }

    /// Return a file document-link at the cursor, or the nearest one on the
    /// same line. The same-line fallback lets `gd` work from the `include`
    /// function name as well as from its quoted path.
    async fn document_link_at(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<types::Location>> {
        let result = self
            .request(
                "textDocument/documentLink",
                json!({
                    "textDocument": { "uri": uri },
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(None);
        }

        let links: Vec<DocumentLink> = serde_json::from_value(result)?;
        Ok(select_document_link(&links, line, character))
    }

    pub async fn references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<types::Location>> {
        let result = self
            .request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": true },
                }),
            )
            .await?;

        parse_locations(result)
    }

    pub async fn code_action(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        end_line: u32,
        end_character: u32,
        diag_json: Value,
    ) -> Result<Vec<types::CodeAction>> {
        let diagnostics: Vec<lsp_types::Diagnostic> = if diag_json.is_array() {
            serde_json::from_value(diag_json).unwrap_or_default()
        } else {
            vec![]
        };

        let result = self
            .request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": line, "character": character },
                        "end": { "line": end_line, "character": end_character },
                    },
                    "context": { "diagnostics": diagnostics },
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        let raw: Vec<Value> = serde_json::from_value(result)?;
        let mut actions = Vec::new();
        let mut cached = Vec::new();

        for (i, item) in raw.into_iter().enumerate() {
            // Could be Command or CodeAction
            if item.get("edit").is_some() || item.get("command").is_some() {
                let title = item
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let kind = item.get("kind").and_then(|k| k.as_str()).map(String::from);
                actions.push(types::CodeAction {
                    title,
                    kind,
                    index: i,
                });
                if let Ok(ca) = serde_json::from_value::<lsp_types::CodeAction>(item) {
                    cached.push(ca);
                } else {
                    cached.push(lsp_types::CodeAction {
                        title: actions.last().unwrap().title.clone(),
                        ..Default::default()
                    });
                }
            } else {
                // It's a Command
                let title = item
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                actions.push(types::CodeAction {
                    title,
                    kind: None,
                    index: i,
                });
                cached.push(lsp_types::CodeAction {
                    title: actions.last().unwrap().title.clone(),
                    ..Default::default()
                });
            }
        }

        *self.cached_actions.lock().await = cached;
        Ok(actions)
    }

    pub async fn execute_code_action(&self, index: usize) -> Result<Option<types::WorkspaceEdit>> {
        let cached = self.cached_actions.lock().await;
        let action = cached
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("invalid action index"))?;

        // Apply workspace edit if present
        let ws_edit = action.edit.as_ref().map(types::from_lsp_workspace_edit);

        // Execute command if present
        if let Some(ref cmd) = action.command {
            let _ = self
                .request(
                    "workspace/executeCommand",
                    json!({
                        "command": cmd.command,
                        "arguments": cmd.arguments,
                    }),
                )
                .await;
        }

        Ok(ws_edit)
    }

    pub async fn formatting(
        &self,
        uri: &str,
        tab_size: u32,
        insert_spaces: bool,
    ) -> Result<Vec<types::TextEdit>> {
        let result = self
            .request(
                "textDocument/formatting",
                json!({
                    "textDocument": { "uri": uri },
                    "options": {
                        "tabSize": tab_size,
                        "insertSpaces": insert_spaces,
                    },
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        let edits: Vec<lsp_types::TextEdit> = serde_json::from_value(result)?;
        Ok(edits
            .iter()
            .map(|e| types::TextEdit {
                line: e.range.start.line,
                character: e.range.start.character,
                end_line: e.range.end.line,
                end_character: e.range.end.character,
                new_text: e.new_text.clone(),
            })
            .collect())
    }

    pub async fn rename(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Option<types::WorkspaceEdit>> {
        let result = self
            .request(
                "textDocument/rename",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "newName": new_name,
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(None);
        }

        let edit: lsp_types::WorkspaceEdit = serde_json::from_value(result)?;
        Ok(Some(types::from_lsp_workspace_edit(&edit)))
    }

    pub async fn signature_help(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<Vec<types::SignatureInfo>>> {
        let result = self
            .request(
                "textDocument/signatureHelp",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(None);
        }

        let sh: SignatureHelp = serde_json::from_value(result)?;
        if sh.signatures.is_empty() {
            return Ok(None);
        }

        let sigs: Vec<types::SignatureInfo> = sh
            .signatures
            .into_iter()
            .enumerate()
            .map(|(_i, sig)| {
                let params: Vec<types::ParameterInfo> = sig
                    .parameters
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| {
                        let label = match p.label {
                            ParameterLabel::Simple(s) => s,
                            ParameterLabel::LabelOffsets([start, end]) => sig
                                .label
                                .get(start as usize..end as usize)
                                .unwrap_or("")
                                .to_string(),
                        };
                        types::ParameterInfo {
                            label,
                            documentation: types::extract_doc(&p.documentation),
                        }
                    })
                    .collect();
                types::SignatureInfo {
                    label: sig.label,
                    documentation: types::extract_doc(&sig.documentation),
                    active_parameter: sig.active_parameter.or(sh.active_parameter),
                    parameters: params,
                }
            })
            .collect();

        Ok(Some(sigs))
    }

    pub async fn implementation(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<types::Location>> {
        let result = self
            .request(
                "textDocument/implementation",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;
        parse_locations(result)
    }

    pub async fn type_definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<types::Location>> {
        let result = self
            .request(
                "textDocument/typeDefinition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;
        parse_locations(result)
    }

    pub async fn document_symbol(&self, uri: &str) -> Result<Vec<types::DocumentSymbolItem>> {
        let result = self
            .request(
                "textDocument/documentSymbol",
                json!({
                    "textDocument": { "uri": uri },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        // Try DocumentSymbol[] first, then SymbolInformation[]
        if let Ok(syms) = serde_json::from_value::<Vec<lsp_types::DocumentSymbol>>(result.clone()) {
            return Ok(convert_doc_symbols(&syms));
        }
        if let Ok(infos) = serde_json::from_value::<Vec<lsp_types::SymbolInformation>>(result) {
            return Ok(infos
                .iter()
                .map(|i| types::DocumentSymbolItem {
                    name: i.name.clone(),
                    kind: types::symbol_kind_label(i.kind).to_string(),
                    detail: None,
                    line: i.location.range.start.line,
                    character: i.location.range.start.character,
                    end_line: i.location.range.end.line,
                    end_character: i.location.range.end.character,
                    children: vec![],
                })
                .collect());
        }
        Ok(vec![])
    }

    pub async fn workspace_symbol(&self, query: &str) -> Result<Vec<types::DocumentSymbolItem>> {
        let result = self
            .request(
                "workspace/symbol",
                json!({
                    "query": query,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        if let Ok(infos) = serde_json::from_value::<Vec<lsp_types::SymbolInformation>>(result) {
            return Ok(infos
                .iter()
                .map(|i| types::DocumentSymbolItem {
                    name: i.name.clone(),
                    kind: types::symbol_kind_label(i.kind).to_string(),
                    detail: Some(i.location.uri.to_string()),
                    line: i.location.range.start.line,
                    character: i.location.range.start.character,
                    end_line: i.location.range.end.line,
                    end_character: i.location.range.end.character,
                    children: vec![],
                })
                .collect());
        }
        Ok(vec![])
    }

    /// Find exact-name declarations in the server's workspace index. This is
    /// used only after a normal definition request fails, notably for Julia
    /// package symbols imported from tests with `using Package: symbol`.
    pub async fn workspace_symbol_locations(&self, query: &str) -> Result<Vec<types::Location>> {
        let result = self
            .request(
                "workspace/symbol",
                json!({
                    "query": query,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }

        let infos: Vec<lsp_types::SymbolInformation> = serde_json::from_value(result)?;
        let mut locations: Vec<_> = infos
            .iter()
            .filter(|info| info.name == query)
            .map(|info| types::from_lsp_location(&info.location))
            .collect();
        locations.sort_by(|a, b| (&a.uri, a.line, a.character).cmp(&(&b.uri, b.line, b.character)));
        locations.dedup_by(|a, b| a.uri == b.uri && a.line == b.line && a.character == b.character);
        Ok(locations)
    }

    pub async fn document_highlight(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<types::DocumentHighlightItem>> {
        let result = self
            .request(
                "textDocument/documentHighlight",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let highlights: Vec<lsp_types::DocumentHighlight> = serde_json::from_value(result)?;
        Ok(highlights
            .iter()
            .map(|h| types::DocumentHighlightItem {
                line: h.range.start.line,
                character: h.range.start.character,
                end_line: h.range.end.line,
                end_character: h.range.end.character,
                kind: types::highlight_kind_label(h.kind).to_string(),
            })
            .collect())
    }

    pub async fn inlay_hints(
        &self,
        uri: &str,
        start_line: u32,
        end_line: u32,
    ) -> Result<Vec<types::InlayHintItem>> {
        let result = self
            .request(
                "textDocument/inlayHint",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": 0 },
                        "end": { "line": end_line, "character": 0 },
                    },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let hints: Vec<lsp_types::InlayHint> = serde_json::from_value(result)?;
        Ok(hints
            .iter()
            .map(|h| {
                let label = match &h.label {
                    lsp_types::InlayHintLabel::String(s) => s.clone(),
                    lsp_types::InlayHintLabel::LabelParts(parts) => parts
                        .iter()
                        .map(|p| p.value.as_str())
                        .collect::<Vec<_>>()
                        .join(""),
                };
                let kind = match h.kind {
                    Some(lsp_types::InlayHintKind::TYPE) => "type",
                    Some(lsp_types::InlayHintKind::PARAMETER) => "parameter",
                    _ => "other",
                };
                types::InlayHintItem {
                    line: h.position.line,
                    character: h.position.character,
                    label,
                    kind: kind.to_string(),
                    padding_left: h.padding_left.unwrap_or(false),
                    padding_right: h.padding_right.unwrap_or(false),
                }
            })
            .collect())
    }

    pub async fn call_hierarchy_prepare(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<lsp_types::CallHierarchyItem>> {
        let result = self
            .request(
                "textDocument/prepareCallHierarchy",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_value(result)?)
    }

    pub async fn call_hierarchy_incoming(
        &self,
        item: &lsp_types::CallHierarchyItem,
    ) -> Result<Vec<types::CallHierarchyCall>> {
        let result = self
            .request(
                "callHierarchy/incomingCalls",
                json!({
                    "item": item,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let calls: Vec<lsp_types::CallHierarchyIncomingCall> = serde_json::from_value(result)?;
        Ok(calls
            .iter()
            .map(|c| types::CallHierarchyCall {
                item: convert_call_hierarchy_item(&c.from),
                from_ranges: c
                    .from_ranges
                    .iter()
                    .map(|r| types::RangeItem {
                        line: r.start.line,
                        character: r.start.character,
                        end_line: r.end.line,
                        end_character: r.end.character,
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn call_hierarchy_outgoing(
        &self,
        item: &lsp_types::CallHierarchyItem,
    ) -> Result<Vec<types::CallHierarchyCall>> {
        let result = self
            .request(
                "callHierarchy/outgoingCalls",
                json!({
                    "item": item,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let calls: Vec<lsp_types::CallHierarchyOutgoingCall> = serde_json::from_value(result)?;
        Ok(calls
            .iter()
            .map(|c| types::CallHierarchyCall {
                item: convert_call_hierarchy_item(&c.to),
                from_ranges: c
                    .from_ranges
                    .iter()
                    .map(|r| types::RangeItem {
                        line: r.start.line,
                        character: r.start.character,
                        end_line: r.end.line,
                        end_character: r.end.character,
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn selection_range(
        &self,
        uri: &str,
        positions: &[(u32, u32)],
    ) -> Result<Vec<types::SelectionRangeItem>> {
        let pos_arr: Vec<_> = positions
            .iter()
            .map(|(l, c)| json!({"line": l, "character": c}))
            .collect();
        let result = self
            .request(
                "textDocument/selectionRange",
                json!({
                    "textDocument": { "uri": uri },
                    "positions": pos_arr,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let ranges: Vec<lsp_types::SelectionRange> = serde_json::from_value(result)?;
        Ok(ranges.iter().map(|r| convert_selection_range(r)).collect())
    }

    async fn get_semtok_legend(&self) -> (Vec<String>, Vec<String>) {
        let caps = self.capabilities.lock().await;
        let legend = caps
            .as_ref()
            .and_then(|c| c.semantic_tokens_provider.as_ref())
            .and_then(|p| match p {
                lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(o) => {
                    Some(&o.legend)
                }
                lsp_types::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
                    o,
                ) => Some(&o.semantic_tokens_options.legend),
            });
        let type_names: Vec<String> = legend
            .map(|l| {
                l.token_types
                    .iter()
                    .map(|t| t.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let mod_names: Vec<String> = legend
            .map(|l| {
                l.token_modifiers
                    .iter()
                    .map(|m| m.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();
        (type_names, mod_names)
    }

    fn decode_raw_tokens(
        data: &[lsp_types::SemanticToken],
        type_names: &[String],
        mod_names: &[String],
    ) -> Vec<types::SemanticTokenItem> {
        let mut decoded = Vec::new();
        let mut line: u32 = 0;
        let mut start: u32 = 0;
        for token in data {
            if token.delta_line > 0 {
                line += token.delta_line;
                start = token.delta_start;
            } else {
                start += token.delta_start;
            }
            let token_type = type_names
                .get(token.token_type as usize)
                .cloned()
                .unwrap_or_else(|| format!("type_{}", token.token_type));
            let mut modifiers = Vec::new();
            for (i, name) in mod_names.iter().enumerate() {
                if token.token_modifiers_bitset & (1 << i) != 0 {
                    modifiers.push(name.clone());
                }
            }
            decoded.push(types::SemanticTokenItem {
                line,
                start,
                length: token.length,
                token_type,
                modifiers,
            });
        }
        decoded
    }

    pub async fn semantic_tokens_full(&self, uri: &str) -> Result<Vec<types::SemanticTokenItem>> {
        let result = self
            .request(
                "textDocument/semanticTokens/full",
                json!({
                    "textDocument": { "uri": uri },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let tokens: lsp_types::SemanticTokens = serde_json::from_value(result)?;
        let (type_names, mod_names) = self.get_semtok_legend().await;

        // Cache result_id and raw data for delta requests
        if let Some(ref id) = tokens.result_id {
            self.semtok_prev_result_id
                .lock()
                .await
                .insert(uri.to_string(), id.clone());
        }
        self.semtok_prev_data
            .lock()
            .await
            .insert(uri.to_string(), tokens.data.clone());

        Ok(Self::decode_raw_tokens(
            &tokens.data,
            &type_names,
            &mod_names,
        ))
    }

    pub async fn semantic_tokens_full_delta(
        &self,
        uri: &str,
    ) -> Result<Vec<types::SemanticTokenItem>> {
        // Check if we have a previous result_id for this URI
        let prev_id = self.semtok_prev_result_id.lock().await.get(uri).cloned();
        let prev_id = match prev_id {
            Some(id) => id,
            None => return self.semantic_tokens_full(uri).await,
        };

        let result = self
            .request(
                "textDocument/semanticTokens/full/delta",
                json!({
                    "textDocument": { "uri": uri },
                    "previousResultId": prev_id,
                }),
            )
            .await;

        // On error, clear cache and fall back to full
        let result = match result {
            Ok(r) => r,
            Err(_) => {
                self.semtok_prev_result_id.lock().await.remove(uri);
                self.semtok_prev_data.lock().await.remove(uri);
                return self.semantic_tokens_full(uri).await;
            }
        };
        if result.is_null() {
            return Ok(vec![]);
        }

        let (type_names, mod_names) = self.get_semtok_legend().await;

        // Try to parse as full response first, then as delta
        if let Ok(full) = serde_json::from_value::<lsp_types::SemanticTokens>(result.clone()) {
            // Server returned full tokens
            if let Some(ref id) = full.result_id {
                self.semtok_prev_result_id
                    .lock()
                    .await
                    .insert(uri.to_string(), id.clone());
            }
            self.semtok_prev_data
                .lock()
                .await
                .insert(uri.to_string(), full.data.clone());
            Ok(Self::decode_raw_tokens(&full.data, &type_names, &mod_names))
        } else if let Ok(delta) = serde_json::from_value::<lsp_types::SemanticTokensDelta>(result) {
            // Server returned delta edits
            if let Some(ref id) = delta.result_id {
                self.semtok_prev_result_id
                    .lock()
                    .await
                    .insert(uri.to_string(), id.clone());
            }

            // Apply edits to cached data
            let mut data_map = self.semtok_prev_data.lock().await;
            let data = data_map.entry(uri.to_string()).or_insert_with(Vec::new);

            // Sort edits by start in reverse order to avoid index shifting
            let mut edits = delta.edits;
            edits.sort_by(|a, b| b.start.cmp(&a.start));

            for edit in &edits {
                let start = edit.start as usize;
                let delete_count = edit.delete_count as usize;
                // Remove old tokens
                let end = (start + delete_count).min(data.len());
                data.drain(start..end);
                // Insert new tokens
                if let Some(ref new_tokens) = edit.data {
                    for (i, token) in new_tokens.iter().enumerate() {
                        data.insert(start + i, token.clone());
                    }
                }
            }

            let decoded = Self::decode_raw_tokens(data, &type_names, &mod_names);
            Ok(decoded)
        } else {
            // Cannot parse response, fall back to full
            self.semtok_prev_result_id.lock().await.remove(uri);
            self.semtok_prev_data.lock().await.remove(uri);
            self.semantic_tokens_full(uri).await
        }
    }

    pub async fn semantic_tokens_range(
        &self,
        uri: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Result<Vec<types::SemanticTokenItem>> {
        let result = self
            .request(
                "textDocument/semanticTokens/range",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": start_char },
                        "end": { "line": end_line, "character": end_char },
                    },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let tokens: lsp_types::SemanticTokens = serde_json::from_value(result)?;
        let (type_names, mod_names) = self.get_semtok_legend().await;
        Ok(Self::decode_raw_tokens(
            &tokens.data,
            &type_names,
            &mod_names,
        ))
    }

    pub async fn code_lens(&self, uri: &str) -> Result<Vec<types::CodeLensItem>> {
        let result = self
            .request(
                "textDocument/codeLens",
                json!({
                    "textDocument": { "uri": uri },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let lenses: Vec<lsp_types::CodeLens> = serde_json::from_value(result)?;
        // Cache for later execution
        *self.cached_code_lenses.lock().await = lenses.clone();
        Ok(lenses
            .iter()
            .enumerate()
            .map(|(idx, l)| types::CodeLensItem {
                line: l.range.start.line,
                character: l.range.start.character,
                end_line: l.range.end.line,
                end_character: l.range.end.character,
                command_title: l.command.as_ref().map(|c| c.title.clone()),
                index: idx,
            })
            .collect())
    }

    pub async fn completion_resolve(
        &self,
        generation: u64,
        index: usize,
    ) -> Result<types::CompletionItem> {
        // Clone before awaiting: holding the cache mutex across an LSP round
        // trip blocks the next completion response from replacing the cache.
        let item = {
            let cached = self.cached_completions.lock().await;
            if cached.generation != generation {
                bail!(
                    "stale completion generation: requested {generation}, current {}",
                    cached.generation
                );
            }
            cached
                .items
                .get(index)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("invalid completion index"))?
        };
        let result = self
            .request_with_timeout(
                "completionItem/resolve",
                serde_json::to_value(&item)?,
                Duration::from_secs(5),
            )
            .await?;
        let mut resolved: lsp_types::CompletionItem = serde_json::from_value(result)?;

        // A resolve response may only fill deferred fields. Preserve edit and
        // insertion data from the original item when the server omits them.
        if resolved.insert_text.is_none() {
            resolved.insert_text = item.insert_text.clone();
        }
        if resolved.insert_text_format.is_none() {
            resolved.insert_text_format = item.insert_text_format;
        }
        if resolved.text_edit.is_none() {
            resolved.text_edit = item.text_edit.clone();
        }
        if resolved.additional_text_edits.is_none() {
            resolved.additional_text_edits = item.additional_text_edits.clone();
        }
        if resolved.commit_characters.is_none() {
            resolved.commit_characters = item.commit_characters.clone();
        }
        if resolved.detail.is_none() {
            resolved.detail = item.detail.clone();
        }
        if resolved.documentation.is_none() {
            resolved.documentation = item.documentation.clone();
        }

        Ok(types::from_lsp_completion_item(&resolved, index))
    }

    pub async fn execute_code_lens(&self, index: usize) -> Result<Option<types::WorkspaceEdit>> {
        let cached = self.cached_code_lenses.lock().await;
        let lens = cached
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("invalid code lens index"))?;
        // Resolve if no command yet
        let lens = if lens.command.is_none() {
            let resolved = self
                .request("codeLens/resolve", serde_json::to_value(lens)?)
                .await?;
            serde_json::from_value::<lsp_types::CodeLens>(resolved)?
        } else {
            lens.clone()
        };
        drop(cached);
        if let Some(ref cmd) = lens.command {
            let result = self
                .request(
                    "workspace/executeCommand",
                    json!({
                        "command": cmd.command,
                        "arguments": cmd.arguments,
                    }),
                )
                .await;
            // Command may return a workspace edit
            if let Ok(val) = result {
                if let Ok(edit) = serde_json::from_value::<lsp_types::WorkspaceEdit>(val) {
                    return Ok(Some(types::from_lsp_workspace_edit(&edit)));
                }
            }
        }
        Ok(None)
    }

    // ─── Type Hierarchy (LSP 3.17) ─────────────────────────

    pub async fn type_hierarchy_prepare(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<lsp_types::TypeHierarchyItem>> {
        let result = self
            .request(
                "textDocument/prepareTypeHierarchy",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_value(result)?)
    }

    pub async fn type_hierarchy_supertypes(
        &self,
        item: &lsp_types::TypeHierarchyItem,
    ) -> Result<Vec<types::CallHierarchyItem>> {
        let result = self
            .request(
                "typeHierarchy/supertypes",
                json!({
                    "item": item,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let items: Vec<lsp_types::TypeHierarchyItem> = serde_json::from_value(result)?;
        Ok(items
            .iter()
            .map(|i| types::CallHierarchyItem {
                name: i.name.clone(),
                kind: types::symbol_kind_label(i.kind).to_string(),
                uri: i.uri.to_string(),
                line: i.selection_range.start.line,
                character: i.selection_range.start.character,
                detail: i.detail.clone(),
            })
            .collect())
    }

    pub async fn type_hierarchy_subtypes(
        &self,
        item: &lsp_types::TypeHierarchyItem,
    ) -> Result<Vec<types::CallHierarchyItem>> {
        let result = self
            .request(
                "typeHierarchy/subtypes",
                json!({
                    "item": item,
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let items: Vec<lsp_types::TypeHierarchyItem> = serde_json::from_value(result)?;
        Ok(items
            .iter()
            .map(|i| types::CallHierarchyItem {
                name: i.name.clone(),
                kind: types::symbol_kind_label(i.kind).to_string(),
                uri: i.uri.to_string(),
                line: i.selection_range.start.line,
                character: i.selection_range.start.character,
                detail: i.detail.clone(),
            })
            .collect())
    }

    // ─── Pull Diagnostics (LSP 3.17) ───────────────────────

    pub async fn pull_diagnostics(&self, uri: &str) -> Result<Vec<types::DiagnosticItem>> {
        let result = self
            .request(
                "textDocument/diagnostic",
                json!({
                    "textDocument": { "uri": uri },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        // Parse DocumentDiagnosticReport
        let items_val = result
            .get("items")
            .or_else(|| result.get("relatedDocuments"))
            .cloned()
            .unwrap_or_else(|| {
                // Try full report format
                result.get("items").cloned().unwrap_or(Value::Array(vec![]))
            });
        if let Ok(diags) = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(items_val) {
            return Ok(diags
                .iter()
                .map(|d| types::DiagnosticItem {
                    line: d.range.start.line,
                    character: d.range.start.character,
                    end_line: d.range.end.line,
                    end_character: d.range.end.character,
                    severity: types::severity_to_u8(d.severity),
                    message: d.message.clone(),
                    source: d.source.clone(),
                    code: d.code.as_ref().map(|c| match c {
                        NumberOrString::Number(n) => n.to_string(),
                        NumberOrString::String(s) => s.clone(),
                    }),
                })
                .collect());
        }
        Ok(vec![])
    }

    pub async fn folding_range(&self, uri: &str) -> Result<Vec<types::FoldingRangeItem>> {
        let result = self
            .request(
                "textDocument/foldingRange",
                json!({
                    "textDocument": { "uri": uri },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(vec![]);
        }
        let ranges: Vec<lsp_types::FoldingRange> = serde_json::from_value(result)?;
        Ok(ranges
            .iter()
            .map(|r| types::FoldingRangeItem {
                start_line: r.start_line,
                end_line: r.end_line,
                kind: r.kind.as_ref().map(|k| match k {
                    lsp_types::FoldingRangeKind::Comment => "comment".to_string(),
                    lsp_types::FoldingRangeKind::Imports => "imports".to_string(),
                    lsp_types::FoldingRangeKind::Region => "region".to_string(),
                }),
            })
            .collect())
    }

    pub async fn linked_editing_range(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<types::LinkedEditingRangeItem>> {
        let result = self
            .request(
                "textDocument/linkedEditingRange",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await?;
        if result.is_null() {
            return Ok(None);
        }
        let ler: lsp_types::LinkedEditingRanges = serde_json::from_value(result)?;
        Ok(Some(types::LinkedEditingRangeItem {
            ranges: ler
                .ranges
                .iter()
                .map(|r| types::RangeItem {
                    line: r.start.line,
                    character: r.start.character,
                    end_line: r.end.line,
                    end_character: r.end.character,
                })
                .collect(),
            word_pattern: ler.word_pattern,
        }))
    }
}

/// Parse GotoDefinitionResponse / locations.
fn parse_locations(result: Value) -> Result<Vec<types::Location>> {
    if result.is_null() {
        return Ok(vec![]);
    }
    // Single location
    if let Ok(loc) = serde_json::from_value::<lsp_types::Location>(result.clone()) {
        return Ok(vec![types::from_lsp_location(&loc)]);
    }
    // Array of locations
    if let Ok(locs) = serde_json::from_value::<Vec<lsp_types::Location>>(result.clone()) {
        return Ok(locs.iter().map(types::from_lsp_location).collect());
    }
    // Array of LocationLink
    if let Ok(links) = serde_json::from_value::<Vec<LocationLink>>(result) {
        return Ok(links
            .iter()
            .map(|l| types::Location {
                uri: types::decode_uri(&l.target_uri.to_string()),
                line: l.target_selection_range.start.line,
                character: l.target_selection_range.start.character,
                end_line: Some(l.target_selection_range.end.line),
                end_character: Some(l.target_selection_range.end.character),
            })
            .collect());
    }
    Ok(vec![])
}

fn select_document_link(
    links: &[DocumentLink],
    line: u32,
    character: u32,
) -> Option<types::Location> {
    links
        .iter()
        .filter_map(|link| {
            let target = link.target.as_ref()?.to_string();
            // Definition navigation opens an editor buffer; web links and
            // other URI schemes do not belong in this fallback.
            if !target.starts_with("file://") {
                return None;
            }

            let range = link.range;
            let contains_cursor = (line > range.start.line
                || (line == range.start.line && character >= range.start.character))
                && (line < range.end.line
                    || (line == range.end.line && character <= range.end.character));
            let is_on_line = line >= range.start.line && line <= range.end.line;
            if !is_on_line {
                return None;
            }

            let distance = if contains_cursor {
                0
            } else if line == range.start.line && character < range.start.character {
                range.start.character - character
            } else if line == range.end.line && character > range.end.character {
                character - range.end.character
            } else {
                0
            };

            Some((
                !contains_cursor,
                distance,
                types::Location {
                    uri: types::decode_uri(&target),
                    line: 0,
                    character: 0,
                    end_line: None,
                    end_character: None,
                },
            ))
        })
        .min_by_key(|(outside, distance, _)| (*outside, *distance))
        .map(|(_, _, location)| location)
}

#[cfg(test)]
mod document_link_tests {
    use super::*;

    fn link(start: u32, end: u32, target: &str) -> DocumentLink {
        DocumentLink {
            range: Range::new(Position::new(4, start), Position::new(4, end)),
            target: Some(target.parse().unwrap()),
            tooltip: None,
            data: None,
        }
    }

    #[test]
    fn selects_link_from_include_function_name_on_same_line() {
        let links = vec![link(8, 29, "file:///tmp/core/transformer.jl")];
        let location = select_document_link(&links, 4, 2).unwrap();
        assert_eq!(location.uri, "/tmp/core/transformer.jl");
        assert_eq!(location.line, 0);
    }

    #[test]
    fn prefers_link_containing_cursor_and_ignores_web_targets() {
        let links = vec![
            link(1, 5, "https://example.com"),
            link(10, 20, "file:///tmp/first.jl"),
            link(25, 35, "file:///tmp/second.jl"),
        ];
        let location = select_document_link(&links, 4, 28).unwrap();
        assert_eq!(location.uri, "/tmp/second.jl");
    }

    #[test]
    fn does_not_select_a_link_from_another_line() {
        let links = vec![link(8, 29, "file:///tmp/core/transformer.jl")];
        assert!(select_document_link(&links, 3, 8).is_none());
    }

    #[test]
    fn decodes_location_link_target_paths() {
        let result = json!([{
            "targetUri": "file:///tmp/My%20Project/source.jl",
            "targetRange": {
                "start": { "line": 1, "character": 0 },
                "end": { "line": 2, "character": 0 }
            },
            "targetSelectionRange": {
                "start": { "line": 1, "character": 4 },
                "end": { "line": 1, "character": 10 }
            }
        }]);
        let locations = parse_locations(result).unwrap();
        assert_eq!(locations[0].uri, "/tmp/My Project/source.jl");
    }
}

fn convert_doc_symbols(syms: &[lsp_types::DocumentSymbol]) -> Vec<types::DocumentSymbolItem> {
    syms.iter()
        .map(|s| types::DocumentSymbolItem {
            name: s.name.clone(),
            kind: types::symbol_kind_label(s.kind).to_string(),
            detail: s.detail.clone(),
            line: s.selection_range.start.line,
            character: s.selection_range.start.character,
            end_line: s.range.end.line,
            end_character: s.range.end.character,
            children: s
                .children
                .as_ref()
                .map(|c| convert_doc_symbols(c))
                .unwrap_or_default(),
        })
        .collect()
}

fn convert_call_hierarchy_item(item: &lsp_types::CallHierarchyItem) -> types::CallHierarchyItem {
    types::CallHierarchyItem {
        name: item.name.clone(),
        kind: types::symbol_kind_label(item.kind).to_string(),
        uri: item.uri.to_string(),
        line: item.selection_range.start.line,
        character: item.selection_range.start.character,
        detail: item.detail.clone(),
    }
}

fn convert_selection_range(r: &lsp_types::SelectionRange) -> types::SelectionRangeItem {
    types::SelectionRangeItem {
        line: r.range.start.line,
        character: r.range.start.character,
        end_line: r.range.end.line,
        end_character: r.range.end.character,
        parent: r
            .parent
            .as_ref()
            .map(|p| Box::new(convert_selection_range(p))),
    }
}

/// Handle server-initiated requests (workspace/applyEdit, etc.)
async fn handle_server_request(
    msg: &Value,
    transport: &Arc<Mutex<LspTransport>>,
    event_tx: &mpsc::Sender<ServerEvent>,
    settings: &Arc<RwLock<Value>>,
    registered_methods: &Arc<Mutex<HashSet<String>>>,
) {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned().unwrap_or(Value::Null);

    match method {
        "workspace/applyEdit" => {
            if let Some(params) = msg.get("params") {
                if let Ok(edit) = serde_json::from_value::<lsp_types::WorkspaceEdit>(
                    params.get("edit").cloned().unwrap_or(Value::Null),
                ) {
                    let ws_edit = types::from_lsp_workspace_edit(&edit);
                    let _ = event_tx
                        .send(ServerEvent::ApplyEdit {
                            id: id.clone(),
                            edit: ws_edit,
                        })
                        .await;
                }
            }
            // Respond with applied=true
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "applied": true },
            });
            let mut t = transport.lock().await;
            let _ = t.send(&resp).await;
        }
        "workspace/configuration" => {
            let result = {
                let settings = settings.read().await;
                configuration_response(msg.get("params"), &settings)
            };
            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let mut t = transport.lock().await;
            let _ = t.send(&resp).await;
        }
        "client/registerCapability" => {
            let methods = registration_methods(msg.get("params"));
            registered_methods.lock().await.extend(methods);
            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null });
            let mut t = transport.lock().await;
            let _ = t.send(&resp).await;
        }
        "window/workDoneProgress/create" => {
            // Acknowledge with null result
            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null });
            let mut t = transport.lock().await;
            let _ = t.send(&resp).await;
        }
        _ => {
            eprintln!("[simplecc] unhandled server request: {method}");
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "method not found" },
            });
            let mut t = transport.lock().await;
            let _ = t.send(&resp).await;
        }
    }
}

fn registration_methods(params: Option<&Value>) -> Vec<String> {
    params
        .and_then(|params| params.get("registrations"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|registration| registration.get("method")?.as_str().map(str::to_owned))
        .collect()
}

fn configuration_response(params: Option<&Value>, settings: &Value) -> Vec<Value> {
    params
        .and_then(|params| params.get("items"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("section")
                        .and_then(Value::as_str)
                        .map(|section| configuration_value(settings, section))
                        .unwrap_or_else(|| settings.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn configuration_value(settings: &Value, section: &str) -> Value {
    // Exact dotted keys allow compact overrides and take precedence over the
    // equivalent path in a nested settings object.
    if let Some(value) = settings.get(section) {
        return value.clone();
    }

    section
        .split('.')
        .try_fold(settings, |value, key| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod configuration_tests {
    use super::*;

    #[test]
    fn returns_configuration_items_in_request_order() {
        let settings = json!({
            "julia": {
                "lint": { "run": true },
                "completionmode": "qualify"
            }
        });
        let params = json!({
            "items": [
                { "section": "julia.completionmode" },
                { "section": "julia.lint.run" },
                { "section": "julia.unknown" }
            ]
        });
        assert_eq!(
            configuration_response(Some(&params), &settings),
            vec![json!("qualify"), json!(true), Value::Null]
        );
    }

    #[test]
    fn exact_dotted_setting_overrides_nested_value() {
        let settings = json!({
            "julia": { "lint": { "run": true } },
            "julia.lint.run": false
        });
        assert_eq!(
            configuration_value(&settings, "julia.lint.run"),
            json!(false)
        );
    }

    #[test]
    fn records_dynamic_registration_methods() {
        let params = json!({
            "registrations": [
                { "id": "files", "method": "workspace/didChangeWatchedFiles" },
                { "id": "config", "method": "workspace/didChangeConfiguration" }
            ]
        });
        assert_eq!(
            registration_methods(Some(&params)),
            vec![
                "workspace/didChangeWatchedFiles".to_string(),
                "workspace/didChangeConfiguration".to_string()
            ]
        );
    }

    #[test]
    fn advertises_dynamic_configuration_updates() {
        let capabilities = client_capabilities();
        assert_eq!(
            capabilities
                .workspace
                .and_then(|workspace| workspace.did_change_configuration)
                .and_then(|configuration| configuration.dynamic_registration),
            Some(true)
        );
    }
}

/// Handle server notifications (diagnostics, log, etc.)
async fn handle_server_notification(
    method: &str,
    msg: &Value,
    event_tx: &mpsc::Sender<ServerEvent>,
) {
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "textDocument/publishDiagnostics" => {
            if let Ok(pd) = serde_json::from_value::<PublishDiagnosticsParams>(params) {
                let items: Vec<types::DiagnosticItem> = pd
                    .diagnostics
                    .iter()
                    .map(|d| types::DiagnosticItem {
                        line: d.range.start.line,
                        character: d.range.start.character,
                        end_line: d.range.end.line,
                        end_character: d.range.end.character,
                        severity: types::severity_to_u8(d.severity),
                        message: d.message.clone(),
                        source: d.source.clone(),
                        code: d.code.as_ref().map(|c| match c {
                            NumberOrString::Number(n) => n.to_string(),
                            NumberOrString::String(s) => s.clone(),
                        }),
                    })
                    .collect();
                let _ = event_tx
                    .send(ServerEvent::Diagnostics {
                        uri: pd.uri.to_string(),
                        diagnostics: items,
                    })
                    .await;
            }
        }
        "window/logMessage" | "window/showMessage" => {
            let level = match params.get("type").and_then(|t| t.as_u64()) {
                Some(1) => "error",
                Some(2) => "warn",
                Some(3) => "info",
                _ => "debug",
            };
            let message = params
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let event = if method == "window/logMessage" {
                ServerEvent::LogMessage {
                    level: level.to_string(),
                    message,
                }
            } else {
                ServerEvent::ShowMessage {
                    level: level.to_string(),
                    message,
                }
            };
            let _ = event_tx.send(event).await;
        }
        "$/progress" => {
            // Forward progress to Vim
            if let Some(token) = params.get("token") {
                if let Some(value) = params.get("value") {
                    let kind = value.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                    let title = value.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let message = value.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    let percentage = value.get("percentage").and_then(|p| p.as_u64());
                    let _ = event_tx
                        .send(ServerEvent::Progress {
                            token: token.to_string(),
                            kind: kind.to_string(),
                            title: title.to_string(),
                            message: message.to_string(),
                            percentage,
                        })
                        .await;
                }
            }
        }
        "window/workDoneProgress" => {}
        _ => {
            // Ignore unknown notifications
        }
    }
}

/// Build client capabilities to advertise to server.
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            completion: Some(CompletionClientCapabilities {
                completion_item: Some(CompletionItemCapability {
                    snippet_support: Some(true),
                    documentation_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                    resolve_support: Some(CompletionItemCapabilityResolveSupport {
                        properties: vec!["documentation".to_string(), "detail".to_string()],
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            hover: Some(HoverClientCapabilities {
                content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                ..Default::default()
            }),
            definition: Some(GotoCapability {
                link_support: Some(true),
                ..Default::default()
            }),
            document_link: Some(DocumentLinkClientCapabilities {
                dynamic_registration: Some(false),
                tooltip_support: Some(true),
            }),
            references: Some(DynamicRegistrationClientCapabilities {
                dynamic_registration: Some(false),
            }),
            signature_help: Some(SignatureHelpClientCapabilities {
                signature_information: Some(SignatureInformationSettings {
                    documentation_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                    parameter_information: Some(ParameterInformationSettings {
                        label_offset_support: Some(true),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            code_action: Some(CodeActionClientCapabilities {
                code_action_literal_support: Some(CodeActionLiteralSupport {
                    code_action_kind: CodeActionKindLiteralSupport {
                        value_set: vec![
                            "quickfix".to_string(),
                            "refactor".to_string(),
                            "refactor.extract".to_string(),
                            "refactor.inline".to_string(),
                            "refactor.rewrite".to_string(),
                            "source".to_string(),
                            "source.organizeImports".to_string(),
                        ],
                    },
                }),
                ..Default::default()
            }),
            formatting: Some(DynamicRegistrationClientCapabilities {
                dynamic_registration: Some(false),
            }),
            rename: Some(RenameClientCapabilities {
                prepare_support: Some(true),
                ..Default::default()
            }),
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                related_information: Some(true),
                ..Default::default()
            }),
            synchronization: Some(TextDocumentSyncClientCapabilities {
                did_save: Some(true),
                ..Default::default()
            }),
            implementation: Some(GotoCapability {
                link_support: Some(true),
                ..Default::default()
            }),
            type_definition: Some(GotoCapability {
                link_support: Some(true),
                ..Default::default()
            }),
            document_symbol: Some(DocumentSymbolClientCapabilities {
                hierarchical_document_symbol_support: Some(true),
                ..Default::default()
            }),
            document_highlight: Some(DocumentHighlightClientCapabilities {
                ..Default::default()
            }),
            inlay_hint: Some(InlayHintClientCapabilities {
                ..Default::default()
            }),
            call_hierarchy: Some(CallHierarchyClientCapabilities {
                ..Default::default()
            }),
            type_hierarchy: Some(TypeHierarchyClientCapabilities {
                ..Default::default()
            }),
            selection_range: Some(SelectionRangeClientCapabilities {
                ..Default::default()
            }),
            semantic_tokens: Some(SemanticTokensClientCapabilities {
                requests: SemanticTokensClientCapabilitiesRequests {
                    full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                    range: Some(true),
                    ..Default::default()
                },
                token_types: vec![
                    SemanticTokenType::NAMESPACE,
                    SemanticTokenType::TYPE,
                    SemanticTokenType::CLASS,
                    SemanticTokenType::ENUM,
                    SemanticTokenType::INTERFACE,
                    SemanticTokenType::STRUCT,
                    SemanticTokenType::TYPE_PARAMETER,
                    SemanticTokenType::PARAMETER,
                    SemanticTokenType::VARIABLE,
                    SemanticTokenType::PROPERTY,
                    SemanticTokenType::ENUM_MEMBER,
                    SemanticTokenType::EVENT,
                    SemanticTokenType::FUNCTION,
                    SemanticTokenType::METHOD,
                    SemanticTokenType::MACRO,
                    SemanticTokenType::KEYWORD,
                    SemanticTokenType::MODIFIER,
                    SemanticTokenType::COMMENT,
                    SemanticTokenType::STRING,
                    SemanticTokenType::NUMBER,
                    SemanticTokenType::REGEXP,
                    SemanticTokenType::OPERATOR,
                    SemanticTokenType::DECORATOR,
                ],
                token_modifiers: vec![
                    SemanticTokenModifier::DECLARATION,
                    SemanticTokenModifier::DEFINITION,
                    SemanticTokenModifier::READONLY,
                    SemanticTokenModifier::STATIC,
                    SemanticTokenModifier::DEPRECATED,
                    SemanticTokenModifier::ABSTRACT,
                    SemanticTokenModifier::ASYNC,
                    SemanticTokenModifier::MODIFICATION,
                    SemanticTokenModifier::DOCUMENTATION,
                    SemanticTokenModifier::DEFAULT_LIBRARY,
                ],
                formats: vec![lsp_types::TokenFormat::RELATIVE],
                ..Default::default()
            }),
            folding_range: Some(FoldingRangeClientCapabilities {
                ..Default::default()
            }),
            linked_editing_range: Some(LinkedEditingRangeClientCapabilities {
                ..Default::default()
            }),
            ..Default::default()
        }),
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..Default::default()
        }),
        workspace: Some(WorkspaceClientCapabilities {
            apply_edit: Some(true),
            workspace_edit: Some(WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                ..Default::default()
            }),
            workspace_folders: Some(true),
            configuration: Some(true),
            did_change_configuration: Some(DynamicRegistrationClientCapabilities {
                dynamic_registration: Some(true),
            }),
            did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                dynamic_registration: Some(true),
                relative_pattern_support: Some(true),
            }),
            symbol: Some(WorkspaceSymbolClientCapabilities {
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}
