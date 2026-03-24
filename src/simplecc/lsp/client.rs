use anyhow::{Result, bail};
use lsp_types::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

use super::transport::LspTransport;
use super::types;

/// A single LSP server client.
pub struct LspClient {
    transport: Arc<Mutex<LspTransport>>,
    next_id: AtomicI64,
    /// Pending requests: jsonrpc id -> oneshot sender
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    /// Server capabilities after initialize
    pub capabilities: Arc<Mutex<Option<ServerCapabilities>>>,
    /// Cached code actions for execute
    pub cached_actions: Arc<Mutex<Vec<lsp_types::CodeAction>>>,
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
    ) -> Result<(Self, mpsc::Receiver<ServerEvent>)> {
        let (transport, mut incoming) =
            LspTransport::spawn(cmd, args, Some(root_path))?;

        let transport = Arc::new(Mutex::new(transport));
        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let capabilities: Arc<Mutex<Option<ServerCapabilities>>> =
            Arc::new(Mutex::new(None));

        let (event_tx, event_rx) = mpsc::channel::<ServerEvent>(256);

        // Spawn message dispatcher
        let pending_clone = pending.clone();
        let transport_clone = transport.clone();
        let event_tx_clone = event_tx.clone();
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
            next_id: AtomicI64::new(1),
            pending,
            capabilities,
            cached_actions: Arc::new(Mutex::new(Vec::new())),
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
        let id = self.next_request_id();
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        {
            let mut t = self.transport.lock().await;
            t.send(&msg).await?;
        }

        let resp = tokio::time::timeout(std::time::Duration::from_secs(120), rx).await??;

        if let Some(err) = resp.get("error") {
            bail!("LSP error: {}", err);
        }

        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut t = self.transport.lock().await;
        t.send(&msg).await
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

    pub async fn did_open(&self, uri: &str, language_id: &str, version: i32, text: &str) -> Result<()> {
        self.notify("textDocument/didOpen", json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": version,
                "text": text,
            }
        })).await
    }

    pub async fn did_change(&self, uri: &str, version: i32, text: &str) -> Result<()> {
        // Full document sync for simplicity
        self.notify("textDocument/didChange", json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [{ "text": text }]
        })).await
    }

    pub async fn did_save(&self, uri: &str, text: Option<&str>) -> Result<()> {
        let mut params = json!({ "textDocument": { "uri": uri } });
        if let Some(t) = text {
            params["text"] = json!(t);
        }
        self.notify("textDocument/didSave", params).await
    }

    pub async fn did_close(&self, uri: &str) -> Result<()> {
        self.notify("textDocument/didClose", json!({
            "textDocument": { "uri": uri }
        })).await
    }

    // ─── LSP Features ───────────────────────────────────────

    pub async fn completion(&self, uri: &str, line: u32, character: u32) -> Result<Vec<types::CompletionItem>> {
        let result = self.request("textDocument/completion", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;

        let items = if result.is_array() {
            serde_json::from_value::<Vec<lsp_types::CompletionItem>>(result)?
        } else if let Ok(list) = serde_json::from_value::<CompletionList>(result.clone()) {
            list.items
        } else {
            vec![]
        };

        Ok(items.into_iter().map(|item| {
            let insert_text = item.insert_text.clone()
                .or_else(|| item.text_edit.as_ref().map(|te| match te {
                    CompletionTextEdit::Edit(e) => e.new_text.clone(),
                    CompletionTextEdit::InsertAndReplace(e) => e.new_text.clone(),
                }));
            types::CompletionItem {
                label: item.label.clone(),
                kind: item.kind.map(types::completion_kind_label).map(String::from),
                detail: item.detail.clone(),
                documentation: types::extract_doc(&item.documentation),
                insert_text,
                sort_text: item.sort_text.clone(),
                filter_text: item.filter_text.clone().or(Some(item.label)),
            }
        }).collect())
    }

    pub async fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<String>> {
        let result = self.request("textDocument/hover", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;

        if result.is_null() {
            return Ok(None);
        }

        let hover: Hover = serde_json::from_value(result)?;
        let text = match hover.contents {
            HoverContents::Scalar(mc) => match mc {
                MarkedString::String(s) => s,
                MarkedString::LanguageString(ls) => format!("```{}\n{}\n```", ls.language, ls.value),
            },
            HoverContents::Array(arr) => arr.into_iter().map(|mc| match mc {
                MarkedString::String(s) => s,
                MarkedString::LanguageString(ls) => format!("```{}\n{}\n```", ls.language, ls.value),
            }).collect::<Vec<_>>().join("\n\n"),
            HoverContents::Markup(mc) => mc.value,
        };
        Ok(Some(text))
    }

    pub async fn definition(&self, uri: &str, line: u32, character: u32) -> Result<Vec<types::Location>> {
        let result = self.request("textDocument/definition", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;

        parse_locations(result)
    }

    pub async fn references(&self, uri: &str, line: u32, character: u32) -> Result<Vec<types::Location>> {
        let result = self.request("textDocument/references", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true },
        })).await?;

        parse_locations(result)
    }

    pub async fn code_action(&self, uri: &str, line: u32, character: u32, end_line: u32, end_character: u32, diag_json: Value) -> Result<Vec<types::CodeAction>> {
        let diagnostics: Vec<lsp_types::Diagnostic> = if diag_json.is_array() {
            serde_json::from_value(diag_json).unwrap_or_default()
        } else {
            vec![]
        };

        let result = self.request("textDocument/codeAction", json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": end_line, "character": end_character },
            },
            "context": { "diagnostics": diagnostics },
        })).await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        let raw: Vec<Value> = serde_json::from_value(result)?;
        let mut actions = Vec::new();
        let mut cached = Vec::new();

        for (i, item) in raw.into_iter().enumerate() {
            // Could be Command or CodeAction
            if item.get("edit").is_some() || item.get("command").is_some() {
                let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let kind = item.get("kind").and_then(|k| k.as_str()).map(String::from);
                actions.push(types::CodeAction { title, kind, index: i });
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
                let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
                actions.push(types::CodeAction { title, kind: None, index: i });
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
        let action = cached.get(index).ok_or_else(|| anyhow::anyhow!("invalid action index"))?;

        // Apply workspace edit if present
        let ws_edit = action.edit.as_ref().map(types::from_lsp_workspace_edit);

        // Execute command if present
        if let Some(ref cmd) = action.command {
            let _ = self.request("workspace/executeCommand", json!({
                "command": cmd.command,
                "arguments": cmd.arguments,
            })).await;
        }

        Ok(ws_edit)
    }

    pub async fn formatting(&self, uri: &str, tab_size: u32, insert_spaces: bool) -> Result<Vec<types::TextEdit>> {
        let result = self.request("textDocument/formatting", json!({
            "textDocument": { "uri": uri },
            "options": {
                "tabSize": tab_size,
                "insertSpaces": insert_spaces,
            },
        })).await?;

        if result.is_null() {
            return Ok(vec![]);
        }

        let edits: Vec<lsp_types::TextEdit> = serde_json::from_value(result)?;
        Ok(edits.iter().map(|e| types::TextEdit {
            line: e.range.start.line,
            character: e.range.start.character,
            end_line: e.range.end.line,
            end_character: e.range.end.character,
            new_text: e.new_text.clone(),
        }).collect())
    }

    pub async fn rename(&self, uri: &str, line: u32, character: u32, new_name: &str) -> Result<Option<types::WorkspaceEdit>> {
        let result = self.request("textDocument/rename", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": new_name,
        })).await?;

        if result.is_null() {
            return Ok(None);
        }

        let edit: lsp_types::WorkspaceEdit = serde_json::from_value(result)?;
        Ok(Some(types::from_lsp_workspace_edit(&edit)))
    }

    pub async fn signature_help(&self, uri: &str, line: u32, character: u32) -> Result<Option<Vec<types::SignatureInfo>>> {
        let result = self.request("textDocument/signatureHelp", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;

        if result.is_null() {
            return Ok(None);
        }

        let sh: SignatureHelp = serde_json::from_value(result)?;
        if sh.signatures.is_empty() {
            return Ok(None);
        }

        let sigs: Vec<types::SignatureInfo> = sh.signatures.into_iter().enumerate().map(|(_i, sig)| {
            let params: Vec<types::ParameterInfo> = sig.parameters.unwrap_or_default().into_iter().map(|p| {
                let label = match p.label {
                    ParameterLabel::Simple(s) => s,
                    ParameterLabel::LabelOffsets([start, end]) => {
                        sig.label.get(start as usize..end as usize).unwrap_or("").to_string()
                    }
                };
                types::ParameterInfo {
                    label,
                    documentation: types::extract_doc(&p.documentation),
                }
            }).collect();
            types::SignatureInfo {
                label: sig.label,
                documentation: types::extract_doc(&sig.documentation),
                active_parameter: sig.active_parameter.or(sh.active_parameter),
                parameters: params,
            }
        }).collect();

        Ok(Some(sigs))
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
        return Ok(links.iter().map(|l| types::Location {
            uri: l.target_uri.to_string(),
            line: l.target_selection_range.start.line,
            character: l.target_selection_range.start.character,
            end_line: Some(l.target_selection_range.end.line),
            end_character: Some(l.target_selection_range.end.character),
        }).collect());
    }
    Ok(vec![])
}

/// Handle server-initiated requests (workspace/applyEdit, etc.)
async fn handle_server_request(
    msg: &Value,
    transport: &Arc<Mutex<LspTransport>>,
    event_tx: &mpsc::Sender<ServerEvent>,
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
                    let _ = event_tx.send(ServerEvent::ApplyEdit { id: id.clone(), edit: ws_edit }).await;
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
        "window/workDoneProgress/create" | "workspace/configuration" | "client/registerCapability" => {
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
                let items: Vec<types::DiagnosticItem> = pd.diagnostics.iter().map(|d| {
                    types::DiagnosticItem {
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
                    }
                }).collect();
                let _ = event_tx.send(ServerEvent::Diagnostics {
                    uri: pd.uri.to_string(),
                    diagnostics: items,
                }).await;
            }
        }
        "window/logMessage" | "window/showMessage" => {
            let level = match params.get("type").and_then(|t| t.as_u64()) {
                Some(1) => "error",
                Some(2) => "warn",
                Some(3) => "info",
                _ => "debug",
            };
            let message = params.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let event = if method == "window/logMessage" {
                ServerEvent::LogMessage { level: level.to_string(), message }
            } else {
                ServerEvent::ShowMessage { level: level.to_string(), message }
            };
            let _ = event_tx.send(event).await;
        }
        "$/progress" | "window/workDoneProgress" => {
            // Silently ignore progress notifications for now
        }
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
                    snippet_support: Some(false),
                    documentation_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
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
            ..Default::default()
        }),
        workspace: Some(WorkspaceClientCapabilities {
            apply_edit: Some(true),
            workspace_edit: Some(WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                ..Default::default()
            }),
            workspace_folders: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
}
