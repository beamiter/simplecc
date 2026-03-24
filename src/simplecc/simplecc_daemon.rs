mod config;
mod installer;
mod lsp;
mod registry;

use anyhow::Result;
use registry::{EventTx, Registry};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use lsp::types;

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
        text: String,
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
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        line: u32, character: u32,
    },
    #[serde(rename = "textDocument/typeDefinition")]
    TypeDefinition {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        line: u32, character: u32,
    },
    #[serde(rename = "textDocument/documentSymbol")]
    DocumentSymbol {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
    },
    #[serde(rename = "workspace/symbol")]
    WorkspaceSymbol {
        id: u64,
        #[serde(rename = "languageId")] language_id: String,
        query: String,
    },
    #[serde(rename = "textDocument/documentHighlight")]
    DocumentHighlight {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        line: u32, character: u32,
    },
    #[serde(rename = "textDocument/inlayHint")]
    InlayHint {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        #[serde(rename = "startLine")] start_line: u32,
        #[serde(rename = "endLine")] end_line: u32,
    },
    #[serde(rename = "textDocument/prepareCallHierarchy")]
    PrepareCallHierarchy {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        line: u32, character: u32,
    },
    #[serde(rename = "callHierarchy/incomingCalls")]
    IncomingCalls {
        id: u64,
        #[serde(rename = "languageId")] language_id: String,
        item: serde_json::Value,
    },
    #[serde(rename = "callHierarchy/outgoingCalls")]
    OutgoingCalls {
        id: u64,
        #[serde(rename = "languageId")] language_id: String,
        item: serde_json::Value,
    },
    #[serde(rename = "textDocument/selectionRange")]
    SelectionRange {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        positions: Vec<serde_json::Value>,
    },
    #[serde(rename = "textDocument/semanticTokens")]
    SemanticTokensFull {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
    },
    #[serde(rename = "textDocument/codeLens")]
    CodeLens {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
    },
    #[serde(rename = "textDocument/foldingRange")]
    FoldingRange {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
    },
    #[serde(rename = "textDocument/linkedEditingRange")]
    LinkedEditingRange {
        id: u64, uri: String,
        #[serde(rename = "languageId")] language_id: String,
        line: u32, character: u32,
    },

    // Server install
    #[serde(rename = "server/install")]
    InstallServer {
        id: u64,
        server: String,
    },
    #[serde(rename = "server/listInstallable")]
    ListInstallable {
        id: u64,
    },
}

fn default_tab_size() -> u32 { 4 }
fn default_true() -> bool { true }

// ─── stdout writer ───────────────────────────────────────

async fn stdout_writer(mut rx: tokio::sync::mpsc::Receiver<String>) {
    let mut out = tokio::io::stdout();
    while let Some(line) = rx.recv().await {
        if out.write_all(line.as_bytes()).await.is_err() { break; }
        if out.write_all(b"\n").await.is_err() { break; }
        let _ = out.flush().await;
    }
}

fn send_event(tx: &EventTx, event: Value) {
    let s = serde_json::to_string(&event).unwrap();
    let _ = tx.try_send(s);
}

// ─── Main ────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    eprintln!("[simplecc] daemon started");

    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(4096);
    tokio::spawn(stdout_writer(out_rx));

    let registry: Arc<Mutex<Option<Registry>>> = Arc::new(Mutex::new(None));
    // Track which filetype a URI belongs to
    let uri_ft: Arc<Mutex<std::collections::HashMap<String, String>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));

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

        tokio::spawn(async move {
            handle_request(req, reg, out, uft).await;
        });
    }

    // Shutdown
    let mut r = registry.lock().await;
    if let Some(ref mut reg) = *r {
        reg.shutdown_all().await;
    }

    eprintln!("[simplecc] daemon exiting");
    Ok(())
}

async fn handle_request(
    req: Request,
    registry: Arc<Mutex<Option<Registry>>>,
    out: EventTx,
    uri_ft: Arc<Mutex<std::collections::HashMap<String, String>>>,
) {
    match req {
        Request::Initialize { id, root, config_path } => {
            let cfg = if let Some(ref p) = config_path {
                let path = std::path::Path::new(p);
                if path.exists() {
                    config::Config::load(path).unwrap_or_else(|_| config::Config::find_and_load(&root))
                } else {
                    config::Config::find_and_load(&root)
                }
            } else {
                config::Config::find_and_load(&root)
            };

            let reg = Registry::new(cfg, root, out.clone());
            *registry.lock().await = Some(reg);

            send_event(&out, json!({"type": "initialized", "id": id}));
        }

        Request::Shutdown { id } => {
            let mut r = registry.lock().await;
            if let Some(ref mut reg) = *r {
                reg.shutdown_all().await;
            }
            send_event(&out, json!({"type": "shutdown", "id": id}));
        }

        Request::DidOpen { id: _, uri, language_id, version, text } => {
            // Track filetype
            uri_ft.lock().await.insert(uri.clone(), language_id.clone());

            let mut r = registry.lock().await;
            if let Some(ref mut reg) = *r {
                // Ensure server started for this filetype
                if let Ok(Some(_name)) = reg.ensure_server(&language_id).await {
                    if let Some(client) = reg.client_for_filetype(&language_id) {
                        let c = client.lock().await;
                        let _ = c.did_open(&uri, &language_id, version, &text).await;
                    }
                }
            }
        }

        Request::DidChange { id: _, uri, version, text } => {
            let ft = uri_ft.lock().await.get(&uri).cloned();
            if let Some(ft) = ft {
                let r = registry.lock().await;
                if let Some(ref reg) = *r {
                    if let Some(client) = reg.client_for_filetype(&ft) {
                        let c = client.lock().await;
                        let _ = c.did_change(&uri, version, &text).await;
                    }
                }
            }
        }

        Request::DidSave { id: _, uri, text } => {
            let ft = uri_ft.lock().await.get(&uri).cloned();
            if let Some(ft) = ft {
                let r = registry.lock().await;
                if let Some(ref reg) = *r {
                    if let Some(client) = reg.client_for_filetype(&ft) {
                        let c = client.lock().await;
                        let _ = c.did_save(&uri, text.as_deref()).await;
                    }
                }
            }
        }

        Request::DidClose { id: _, uri } => {
            let ft = uri_ft.lock().await.remove(&uri);
            if let Some(ft) = ft {
                let r = registry.lock().await;
                if let Some(ref reg) = *r {
                    if let Some(client) = reg.client_for_filetype(&ft) {
                        let c = client.lock().await;
                        let _ = c.did_close(&uri).await;
                    }
                }
            }
        }

        Request::Completion { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.completion(&uri, line, character).await {
                        Ok(items) => send_event(&out, json!({"type": "completion", "id": id, "items": items})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                } else {
                    send_event(&out, json!({"type": "completion", "id": id, "items": []}));
                }
            }
        }

        Request::Hover { id, uri, language_id, line, character } => {
            eprintln!("[simplecc] hover request: uri={} lang={} line={} char={}", uri, language_id, line, character);
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.hover(&uri, line, character).await {
                        Ok(Some(contents)) => {
                            eprintln!("[simplecc] hover result: {} bytes", contents.len());
                            send_event(&out, json!({"type": "hover", "id": id, "contents": contents}));
                        }
                        Ok(None) => {
                            eprintln!("[simplecc] hover result: none");
                            send_event(&out, json!({"type": "hover", "id": id, "contents": null}));
                        }
                        Err(e) => {
                            eprintln!("[simplecc] hover error: {}", e);
                            send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()}));
                        }
                    }
                } else {
                    eprintln!("[simplecc] hover: no client for filetype: {}", language_id);
                }
            }
        }

        Request::Definition { id, uri, language_id, line, character } => {
            eprintln!("[simplecc] definition request: uri={} lang={} line={} char={}", uri, language_id, line, character);
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.definition(&uri, line, character).await {
                        Ok(locs) => {
                            eprintln!("[simplecc] definition result: {} locations", locs.len());
                            send_event(&out, json!({"type": "definition", "id": id, "locations": locs}));
                        }
                        Err(e) => {
                            eprintln!("[simplecc] definition error: {}", e);
                            send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()}));
                        }
                    }
                } else {
                    eprintln!("[simplecc] no client for filetype: {}", language_id);
                }
            } else {
                eprintln!("[simplecc] registry not initialized");
            }
        }

        Request::References { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.references(&uri, line, character).await {
                        Ok(locs) => send_event(&out, json!({"type": "references", "id": id, "locations": locs})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::CodeAction { id, uri, language_id, line, character, end_line, end_character, diagnostics } => {
            let el = end_line.unwrap_or(line);
            let ec = end_character.unwrap_or(character);
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.code_action(&uri, line, character, el, ec, diagnostics).await {
                        Ok(actions) => send_event(&out, json!({"type": "codeAction", "id": id, "actions": actions})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::ExecuteAction { id, language_id, index } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.execute_code_action(index).await {
                        Ok(Some(edit)) => send_event(&out, json!({"type": "applyEdit", "id": id, "edit": edit})),
                        Ok(None) => send_event(&out, json!({"type": "executeAction", "id": id})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::Formatting { id, uri, language_id, tab_size, insert_spaces } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.formatting(&uri, tab_size, insert_spaces).await {
                        Ok(edits) => send_event(&out, json!({"type": "formatting", "id": id, "edits": edits})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::Rename { id, uri, language_id, line, character, new_name } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.rename(&uri, line, character, &new_name).await {
                        Ok(Some(edit)) => send_event(&out, json!({"type": "rename", "id": id, "edit": edit})),
                        Ok(None) => send_event(&out, json!({"type": "rename", "id": id, "edit": null})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::SignatureHelp { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.signature_help(&uri, line, character).await {
                        Ok(Some(sigs)) => send_event(&out, json!({"type": "signatureHelp", "id": id, "signatures": sigs})),
                        Ok(None) => send_event(&out, json!({"type": "signatureHelp", "id": id, "signatures": null})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::Implementation { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.implementation(&uri, line, character).await {
                        Ok(locs) => send_event(&out, json!({"type": "implementation", "id": id, "locations": locs})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::TypeDefinition { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.type_definition(&uri, line, character).await {
                        Ok(locs) => send_event(&out, json!({"type": "typeDefinition", "id": id, "locations": locs})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::DocumentSymbol { id, uri, language_id } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.document_symbol(&uri).await {
                        Ok(symbols) => send_event(&out, json!({"type": "documentSymbol", "id": id, "symbols": symbols})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::WorkspaceSymbol { id, language_id, query } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.workspace_symbol(&query).await {
                        Ok(symbols) => send_event(&out, json!({"type": "workspaceSymbol", "id": id, "symbols": symbols})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::DocumentHighlight { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.document_highlight(&uri, line, character).await {
                        Ok(highlights) => send_event(&out, json!({"type": "documentHighlight", "id": id, "highlights": highlights})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::InlayHint { id, uri, language_id, start_line, end_line } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.inlay_hints(&uri, start_line, end_line).await {
                        Ok(hints) => send_event(&out, json!({"type": "inlayHint", "id": id, "hints": hints})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::PrepareCallHierarchy { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.call_hierarchy_prepare(&uri, line, character).await {
                        Ok(items) => {
                            let converted: Vec<_> = items.iter().map(|i| json!({
                                "name": i.name,
                                "kind": types::symbol_kind_label(i.kind),
                                "uri": i.uri.to_string(),
                                "line": i.selection_range.start.line,
                                "character": i.selection_range.start.character,
                                "detail": i.detail,
                                "raw": serde_json::to_value(i).ok(),
                            })).collect();
                            send_event(&out, json!({"type": "callHierarchyPrepare", "id": id, "items": converted}));
                        }
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::IncomingCalls { id, language_id, item } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    if let Ok(lsp_item) = serde_json::from_value::<lsp_types::CallHierarchyItem>(item) {
                        match c.call_hierarchy_incoming(&lsp_item).await {
                            Ok(calls) => send_event(&out, json!({"type": "incomingCalls", "id": id, "calls": calls})),
                            Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                        }
                    }
                }
            }
        }

        Request::OutgoingCalls { id, language_id, item } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    if let Ok(lsp_item) = serde_json::from_value::<lsp_types::CallHierarchyItem>(item) {
                        match c.call_hierarchy_outgoing(&lsp_item).await {
                            Ok(calls) => send_event(&out, json!({"type": "outgoingCalls", "id": id, "calls": calls})),
                            Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                        }
                    }
                }
            }
        }

        Request::SelectionRange { id, uri, language_id, positions } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    let pos: Vec<(u32, u32)> = positions.iter().filter_map(|p| {
                        Some((p.get("line")?.as_u64()? as u32, p.get("character")?.as_u64()? as u32))
                    }).collect();
                    match c.selection_range(&uri, &pos).await {
                        Ok(ranges) => send_event(&out, json!({"type": "selectionRange", "id": id, "ranges": ranges})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::SemanticTokensFull { id, uri, language_id } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.semantic_tokens_full(&uri).await {
                        Ok(tokens) => send_event(&out, json!({"type": "semanticTokens", "id": id, "tokens": tokens})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::CodeLens { id, uri, language_id } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.code_lens(&uri).await {
                        Ok(lenses) => send_event(&out, json!({"type": "codeLens", "id": id, "lenses": lenses})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::FoldingRange { id, uri, language_id } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.folding_range(&uri).await {
                        Ok(ranges) => send_event(&out, json!({"type": "foldingRange", "id": id, "ranges": ranges})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
                }
            }
        }

        Request::LinkedEditingRange { id, uri, language_id, line, character } => {
            let r = registry.lock().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client.lock().await;
                    match c.linked_editing_range(&uri, line, character).await {
                        Ok(Some(ranges)) => send_event(&out, json!({"type": "linkedEditingRange", "id": id, "result": ranges})),
                        Ok(None) => send_event(&out, json!({"type": "linkedEditingRange", "id": id, "result": null})),
                        Err(e) => send_event(&out, json!({"type": "error", "id": id, "message": e.to_string()})),
                    }
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
                        send_event(&out_clone, json!({
                            "type": "installResult",
                            "id": id,
                            "server": server,
                            "status": "ok",
                            "path": path.to_string_lossy(),
                        }));
                        // Update registry so the server can be started with the new path
                        let mut r = reg_clone.lock().await;
                        if let Some(ref mut reg) = *r {
                            reg.update_server_command(&server, &path.to_string_lossy());
                        }
                    }
                    Err(e) => {
                        eprintln!("[simplecc] install {} failed: {}", server, e);
                        send_event(&out_clone, json!({
                            "type": "installResult",
                            "id": id,
                            "server": server,
                            "status": "error",
                            "message": e.to_string(),
                        }));
                    }
                }
            });
        }

        Request::ListInstallable { id } => {
            let servers = installer::list_installable();
            send_event(&out, json!({
                "type": "installableServers",
                "id": id,
                "servers": servers,
            }));
        }
    }
}
