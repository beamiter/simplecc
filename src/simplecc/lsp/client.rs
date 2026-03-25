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
    /// Cached completion items for resolve
    pub cached_completions: Arc<Mutex<Vec<lsp_types::CompletionItem>>>,
    /// Cached code lenses for execute
    pub cached_code_lenses: Arc<Mutex<Vec<lsp_types::CodeLens>>>,
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
            cached_completions: Arc::new(Mutex::new(Vec::new())),
            cached_code_lenses: Arc::new(Mutex::new(Vec::new())),
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

    pub async fn did_change(&self, uri: &str, version: i32, text: Option<&str>, changes: Option<Vec<Value>>) -> Result<()> {
        let content_changes = if let Some(changes) = changes {
            // Check if server supports incremental sync
            let caps = self.capabilities.lock().await;
            let supports_incremental = caps.as_ref().and_then(|c| {
                match &c.text_document_sync {
                    Some(TextDocumentSyncCapability::Kind(kind)) => Some(*kind == TextDocumentSyncKind::INCREMENTAL),
                    Some(TextDocumentSyncCapability::Options(opts)) => opts.change.map(|k| k == TextDocumentSyncKind::INCREMENTAL),
                    None => None,
                }
            }).unwrap_or(false);
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

        self.notify("textDocument/didChange", json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": content_changes
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

        // Cache raw items for resolve
        *self.cached_completions.lock().await = items.clone();

        Ok(items.into_iter().enumerate().map(|(idx, item)| {
            let is_snippet = item.insert_text_format == Some(InsertTextFormat::SNIPPET);
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
                index: idx,
                is_snippet: if is_snippet { Some(true) } else { None },
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

    pub async fn implementation(&self, uri: &str, line: u32, character: u32) -> Result<Vec<types::Location>> {
        let result = self.request("textDocument/implementation", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;
        parse_locations(result)
    }

    pub async fn type_definition(&self, uri: &str, line: u32, character: u32) -> Result<Vec<types::Location>> {
        let result = self.request("textDocument/typeDefinition", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;
        parse_locations(result)
    }

    pub async fn document_symbol(&self, uri: &str) -> Result<Vec<types::DocumentSymbolItem>> {
        let result = self.request("textDocument/documentSymbol", json!({
            "textDocument": { "uri": uri },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        // Try DocumentSymbol[] first, then SymbolInformation[]
        if let Ok(syms) = serde_json::from_value::<Vec<lsp_types::DocumentSymbol>>(result.clone()) {
            return Ok(convert_doc_symbols(&syms));
        }
        if let Ok(infos) = serde_json::from_value::<Vec<lsp_types::SymbolInformation>>(result) {
            return Ok(infos.iter().map(|i| types::DocumentSymbolItem {
                name: i.name.clone(),
                kind: types::symbol_kind_label(i.kind).to_string(),
                detail: None,
                line: i.location.range.start.line,
                character: i.location.range.start.character,
                end_line: i.location.range.end.line,
                end_character: i.location.range.end.character,
                children: vec![],
            }).collect());
        }
        Ok(vec![])
    }

    pub async fn workspace_symbol(&self, query: &str) -> Result<Vec<types::DocumentSymbolItem>> {
        let result = self.request("workspace/symbol", json!({
            "query": query,
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        if let Ok(infos) = serde_json::from_value::<Vec<lsp_types::SymbolInformation>>(result) {
            return Ok(infos.iter().map(|i| types::DocumentSymbolItem {
                name: i.name.clone(),
                kind: types::symbol_kind_label(i.kind).to_string(),
                detail: Some(i.location.uri.to_string()),
                line: i.location.range.start.line,
                character: i.location.range.start.character,
                end_line: i.location.range.end.line,
                end_character: i.location.range.end.character,
                children: vec![],
            }).collect());
        }
        Ok(vec![])
    }

    pub async fn document_highlight(&self, uri: &str, line: u32, character: u32) -> Result<Vec<types::DocumentHighlightItem>> {
        let result = self.request("textDocument/documentHighlight", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let highlights: Vec<lsp_types::DocumentHighlight> = serde_json::from_value(result)?;
        Ok(highlights.iter().map(|h| types::DocumentHighlightItem {
            line: h.range.start.line,
            character: h.range.start.character,
            end_line: h.range.end.line,
            end_character: h.range.end.character,
            kind: types::highlight_kind_label(h.kind).to_string(),
        }).collect())
    }

    pub async fn inlay_hints(&self, uri: &str, start_line: u32, end_line: u32) -> Result<Vec<types::InlayHintItem>> {
        let result = self.request("textDocument/inlayHint", json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": start_line, "character": 0 },
                "end": { "line": end_line, "character": 0 },
            },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let hints: Vec<lsp_types::InlayHint> = serde_json::from_value(result)?;
        Ok(hints.iter().map(|h| {
            let label = match &h.label {
                lsp_types::InlayHintLabel::String(s) => s.clone(),
                lsp_types::InlayHintLabel::LabelParts(parts) => {
                    parts.iter().map(|p| p.value.as_str()).collect::<Vec<_>>().join("")
                }
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
        }).collect())
    }

    pub async fn call_hierarchy_prepare(&self, uri: &str, line: u32, character: u32) -> Result<Vec<lsp_types::CallHierarchyItem>> {
        let result = self.request("textDocument/prepareCallHierarchy", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        Ok(serde_json::from_value(result)?)
    }

    pub async fn call_hierarchy_incoming(&self, item: &lsp_types::CallHierarchyItem) -> Result<Vec<types::CallHierarchyCall>> {
        let result = self.request("callHierarchy/incomingCalls", json!({
            "item": item,
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let calls: Vec<lsp_types::CallHierarchyIncomingCall> = serde_json::from_value(result)?;
        Ok(calls.iter().map(|c| types::CallHierarchyCall {
            item: convert_call_hierarchy_item(&c.from),
            from_ranges: c.from_ranges.iter().map(|r| types::RangeItem {
                line: r.start.line, character: r.start.character,
                end_line: r.end.line, end_character: r.end.character,
            }).collect(),
        }).collect())
    }

    pub async fn call_hierarchy_outgoing(&self, item: &lsp_types::CallHierarchyItem) -> Result<Vec<types::CallHierarchyCall>> {
        let result = self.request("callHierarchy/outgoingCalls", json!({
            "item": item,
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let calls: Vec<lsp_types::CallHierarchyOutgoingCall> = serde_json::from_value(result)?;
        Ok(calls.iter().map(|c| types::CallHierarchyCall {
            item: convert_call_hierarchy_item(&c.to),
            from_ranges: c.from_ranges.iter().map(|r| types::RangeItem {
                line: r.start.line, character: r.start.character,
                end_line: r.end.line, end_character: r.end.character,
            }).collect(),
        }).collect())
    }

    pub async fn selection_range(&self, uri: &str, positions: &[(u32, u32)]) -> Result<Vec<types::SelectionRangeItem>> {
        let pos_arr: Vec<_> = positions.iter().map(|(l, c)| json!({"line": l, "character": c})).collect();
        let result = self.request("textDocument/selectionRange", json!({
            "textDocument": { "uri": uri },
            "positions": pos_arr,
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let ranges: Vec<lsp_types::SelectionRange> = serde_json::from_value(result)?;
        Ok(ranges.iter().map(|r| convert_selection_range(r)).collect())
    }

    pub async fn semantic_tokens_full(&self, uri: &str) -> Result<Vec<types::SemanticTokenItem>> {
        let result = self.request("textDocument/semanticTokens/full", json!({
            "textDocument": { "uri": uri },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let tokens: lsp_types::SemanticTokens = serde_json::from_value(result)?;
        let caps = self.capabilities.lock().await;
        let legend = caps.as_ref()
            .and_then(|c| c.semantic_tokens_provider.as_ref())
            .and_then(|p| match p {
                lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(o) => Some(&o.legend),
                lsp_types::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(o) => Some(&o.semantic_tokens_options.legend),
            });
        let type_names: Vec<String> = legend.map(|l| l.token_types.iter().map(|t| t.as_str().to_string()).collect()).unwrap_or_default();
        let mod_names: Vec<String> = legend.map(|l| l.token_modifiers.iter().map(|m| m.as_str().to_string()).collect()).unwrap_or_default();
        drop(caps);

        let mut decoded = Vec::new();
        let mut line: u32 = 0;
        let mut start: u32 = 0;
        for token in tokens.data {
            if token.delta_line > 0 {
                line += token.delta_line;
                start = token.delta_start;
            } else {
                start += token.delta_start;
            }
            let token_type = type_names.get(token.token_type as usize).cloned().unwrap_or_else(|| format!("type_{}", token.token_type));
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
        Ok(decoded)
    }

    pub async fn code_lens(&self, uri: &str) -> Result<Vec<types::CodeLensItem>> {
        let result = self.request("textDocument/codeLens", json!({
            "textDocument": { "uri": uri },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let lenses: Vec<lsp_types::CodeLens> = serde_json::from_value(result)?;
        // Cache for later execution
        *self.cached_code_lenses.lock().await = lenses.clone();
        Ok(lenses.iter().enumerate().map(|(idx, l)| types::CodeLensItem {
            line: l.range.start.line,
            character: l.range.start.character,
            end_line: l.range.end.line,
            end_character: l.range.end.character,
            command_title: l.command.as_ref().map(|c| c.title.clone()),
            index: idx,
        }).collect())
    }

    pub async fn completion_resolve(&self, index: usize) -> Result<types::CompletionItem> {
        let cached = self.cached_completions.lock().await;
        let item = cached.get(index).ok_or_else(|| anyhow::anyhow!("invalid completion index"))?;
        let result = self.request("completionItem/resolve", serde_json::to_value(item)?).await?;
        let resolved: lsp_types::CompletionItem = serde_json::from_value(result)?;
        Ok(types::CompletionItem {
            label: resolved.label.clone(),
            kind: resolved.kind.map(types::completion_kind_label).map(String::from),
            detail: resolved.detail.clone(),
            documentation: types::extract_doc(&resolved.documentation),
            insert_text: resolved.insert_text.clone(),
            sort_text: resolved.sort_text.clone(),
            filter_text: resolved.filter_text.clone(),
            index,
            is_snippet: None,
        })
    }

    pub async fn execute_code_lens(&self, index: usize) -> Result<Option<types::WorkspaceEdit>> {
        let cached = self.cached_code_lenses.lock().await;
        let lens = cached.get(index).ok_or_else(|| anyhow::anyhow!("invalid code lens index"))?;
        // Resolve if no command yet
        let lens = if lens.command.is_none() {
            let resolved = self.request("codeLens/resolve", serde_json::to_value(lens)?).await?;
            serde_json::from_value::<lsp_types::CodeLens>(resolved)?
        } else {
            lens.clone()
        };
        drop(cached);
        if let Some(ref cmd) = lens.command {
            let result = self.request("workspace/executeCommand", json!({
                "command": cmd.command,
                "arguments": cmd.arguments,
            })).await;
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

    pub async fn type_hierarchy_prepare(&self, uri: &str, line: u32, character: u32) -> Result<Vec<lsp_types::TypeHierarchyItem>> {
        let result = self.request("textDocument/prepareTypeHierarchy", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        Ok(serde_json::from_value(result)?)
    }

    pub async fn type_hierarchy_supertypes(&self, item: &lsp_types::TypeHierarchyItem) -> Result<Vec<types::CallHierarchyItem>> {
        let result = self.request("typeHierarchy/supertypes", json!({
            "item": item,
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let items: Vec<lsp_types::TypeHierarchyItem> = serde_json::from_value(result)?;
        Ok(items.iter().map(|i| types::CallHierarchyItem {
            name: i.name.clone(),
            kind: types::symbol_kind_label(i.kind).to_string(),
            uri: i.uri.to_string(),
            line: i.selection_range.start.line,
            character: i.selection_range.start.character,
            detail: i.detail.clone(),
        }).collect())
    }

    pub async fn type_hierarchy_subtypes(&self, item: &lsp_types::TypeHierarchyItem) -> Result<Vec<types::CallHierarchyItem>> {
        let result = self.request("typeHierarchy/subtypes", json!({
            "item": item,
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let items: Vec<lsp_types::TypeHierarchyItem> = serde_json::from_value(result)?;
        Ok(items.iter().map(|i| types::CallHierarchyItem {
            name: i.name.clone(),
            kind: types::symbol_kind_label(i.kind).to_string(),
            uri: i.uri.to_string(),
            line: i.selection_range.start.line,
            character: i.selection_range.start.character,
            detail: i.detail.clone(),
        }).collect())
    }

    // ─── Pull Diagnostics (LSP 3.17) ───────────────────────

    pub async fn pull_diagnostics(&self, uri: &str) -> Result<Vec<types::DiagnosticItem>> {
        let result = self.request("textDocument/diagnostic", json!({
            "textDocument": { "uri": uri },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        // Parse DocumentDiagnosticReport
        let items_val = result.get("items")
            .or_else(|| result.get("relatedDocuments"))
            .cloned()
            .unwrap_or_else(|| {
                // Try full report format
                result.get("items").cloned().unwrap_or(Value::Array(vec![]))
            });
        if let Ok(diags) = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(items_val) {
            return Ok(diags.iter().map(|d| types::DiagnosticItem {
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
            }).collect());
        }
        Ok(vec![])
    }

    pub async fn folding_range(&self, uri: &str) -> Result<Vec<types::FoldingRangeItem>> {
        let result = self.request("textDocument/foldingRange", json!({
            "textDocument": { "uri": uri },
        })).await?;
        if result.is_null() { return Ok(vec![]); }
        let ranges: Vec<lsp_types::FoldingRange> = serde_json::from_value(result)?;
        Ok(ranges.iter().map(|r| types::FoldingRangeItem {
            start_line: r.start_line,
            end_line: r.end_line,
            kind: r.kind.as_ref().map(|k| match k {
                lsp_types::FoldingRangeKind::Comment => "comment".to_string(),
                lsp_types::FoldingRangeKind::Imports => "imports".to_string(),
                lsp_types::FoldingRangeKind::Region => "region".to_string(),
                _ => "other".to_string(),
            }),
        }).collect())
    }

    pub async fn linked_editing_range(&self, uri: &str, line: u32, character: u32) -> Result<Option<types::LinkedEditingRangeItem>> {
        let result = self.request("textDocument/linkedEditingRange", json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        })).await?;
        if result.is_null() { return Ok(None); }
        let ler: lsp_types::LinkedEditingRanges = serde_json::from_value(result)?;
        Ok(Some(types::LinkedEditingRangeItem {
            ranges: ler.ranges.iter().map(|r| types::RangeItem {
                line: r.start.line, character: r.start.character,
                end_line: r.end.line, end_character: r.end.character,
            }).collect(),
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

fn convert_doc_symbols(syms: &[lsp_types::DocumentSymbol]) -> Vec<types::DocumentSymbolItem> {
    syms.iter().map(|s| types::DocumentSymbolItem {
        name: s.name.clone(),
        kind: types::symbol_kind_label(s.kind).to_string(),
        detail: s.detail.clone(),
        line: s.selection_range.start.line,
        character: s.selection_range.start.character,
        end_line: s.range.end.line,
        end_character: s.range.end.character,
        children: s.children.as_ref().map(|c| convert_doc_symbols(c)).unwrap_or_default(),
    }).collect()
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
        parent: r.parent.as_ref().map(|p| Box::new(convert_selection_range(p))),
    }
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
        "$/progress" => {
            // Forward progress to Vim
            if let Some(token) = params.get("token") {
                if let Some(value) = params.get("value") {
                    let kind = value.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                    let title = value.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let message = value.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    let percentage = value.get("percentage").and_then(|p| p.as_u64());
                    let _ = event_tx.send(ServerEvent::Progress {
                        token: token.to_string(),
                        kind: kind.to_string(),
                        title: title.to_string(),
                        message: message.to_string(),
                        percentage,
                    }).await;
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
                    full: Some(SemanticTokensFullOptions::Bool(true)),
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
            symbol: Some(WorkspaceSymbolClientCapabilities {
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}
