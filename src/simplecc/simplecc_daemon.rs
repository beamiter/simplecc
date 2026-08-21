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
use serde_json::{Value, json};
use std::io::Write as StdWrite;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use workspace_watcher::WorkspaceWatcher;

/// Per-keystroke request logging is opt-in: set SIMPLECC_DEBUG=1.
fn debug_enabled() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("SIMPLECC_DEBUG").is_ok_and(|value| !value.is_empty() && value != "0")
    })
}

macro_rules! debug_log {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!($($arg)*);
        }
    };
}

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
        #[serde(default)]
        remote: Option<config::RemoteConfig>,
        #[serde(default)]
        remote_config: Option<String>,
        #[serde(default)]
        python_path: String,
        #[serde(default)]
        python_lsp_path: String,
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
        /// The remote workspace's simplecc.json, verbatim, when SimpleRemote
        /// is connected — the same text `initialize` carries in
        /// `remote_config`.  Without it a reload would search the local
        /// filesystem for a file that lives on the other host.
        #[serde(rename = "remoteConfig", default)]
        remote_config: Option<String>,
    },
    /// Files created, changed or deleted outside a buffer write.  The daemon
    /// runs no filesystem watcher for a remote workspace, so SimpleRemote's
    /// SimpleRemoteFilesChanged event is forwarded through this request.
    #[serde(rename = "workspace/didChangeWatchedFiles")]
    DidChangeWatchedFiles {
        #[serde(default)]
        id: u64,
        changes: Vec<WatchedFileChange>,
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
        /// Honour the server's `sortText` ranking before truncating to
        /// `maxItems`. Defaults on so an older Vim layer that never sends the
        /// field still gets the ranked list.
        #[serde(rename = "sortItems", default = "default_true")]
        sort_items: bool,
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
    #[serde(rename = "textDocument/rangeFormatting")]
    RangeFormatting {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
        end_line: u32,
        end_character: u32,
        #[serde(default = "default_tab_size")]
        tab_size: u32,
        #[serde(default = "default_true")]
        insert_spaces: bool,
    },
    #[serde(rename = "textDocument/prepareRename")]
    PrepareRename {
        id: u64,
        uri: String,
        #[serde(rename = "languageId")]
        language_id: String,
        line: u32,
        character: u32,
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

    /// Editor answer to a server-initiated request that was forwarded to Vim
    /// (workspace/applyEdit outcomes, window/showMessageRequest choices).
    #[serde(rename = "server/response")]
    ServerResponse {
        #[serde(default)]
        id: u64,
        server: String,
        #[serde(rename = "requestId")]
        request_id: Value,
        #[serde(default)]
        result: Value,
    },
}

/// One entry of `workspace/didChangeWatchedFiles`: an LSP FileChangeType
/// (1 created, 2 changed, 3 deleted) for a document URI.
#[derive(Debug, Deserialize)]
struct WatchedFileChange {
    uri: String,
    #[serde(rename = "type", default = "default_watched_file_change_type")]
    change_type: u32,
}

fn default_watched_file_change_type() -> u32 {
    2
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
                // A rename delivered as deleted+created must reach the
                // server after the didClose/didOpen of the buffers it moved.
                | Self::DidChangeWatchedFiles { .. }
                // A pending language server is blocked until its answer
                // arrives; never queue it behind slow feature tasks.
                | Self::ServerResponse { .. }
        )
    }

    fn is_lifecycle_barrier(&self) -> bool {
        matches!(self, Self::Initialize { .. } | Self::Shutdown { .. })
    }

    fn is_install(&self) -> bool {
        matches!(self, Self::InstallServer { .. })
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

/// A didOpen/didChange request carries the whole source buffer in one JSONL
/// record. Keep that record large enough for real source files but finite: a
/// client that loses its newline must not grow the daemon until the machine
/// runs out of memory.
const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024 * 1024;

fn finish_request_line(mut bytes: Vec<u8>, too_long: bool, limit: usize) -> Result<String, String> {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if too_long || bytes.len() > limit {
        return Err(format!("request line exceeds {limit} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| "request line is not valid UTF-8".to_string())
}

/// Read one bounded JSONL record and discard the remainder of an oversized
/// record through its newline. The next valid request can then still be
/// processed; `AsyncBufReadExt::lines()` cannot provide either guarantee.
async fn read_request_line<R>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<Result<String, String>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut too_long = false;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if bytes.is_empty() && !too_long {
                Ok(None)
            } else {
                Ok(Some(finish_request_line(bytes, too_long, limit)))
            };
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_len = newline.unwrap_or(available.len());
        let consumed = newline.map_or(available.len(), |position| position + 1);
        if !too_long {
            // CR in CRLF is framing, so retain one extra byte until the record
            // is finished and strip it before enforcing the documented limit.
            if bytes.len().saturating_add(content_len) > limit.saturating_add(1) {
                bytes.clear();
                too_long = true;
            } else {
                bytes.extend_from_slice(&available[..content_len]);
            }
        }
        reader.consume(consumed);

        if newline.is_some() {
            return Ok(Some(finish_request_line(bytes, too_long, limit)));
        }
    }
}

fn stdout_writer(mut rx: tokio::sync::mpsc::Receiver<String>) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    while let Some(line) = rx.blocking_recv() {
        if out.write_all(line.as_bytes()).is_err() {
            break;
        }
        if out.write_all(b"\n").is_err() {
            break;
        }
        let _ = out.flush();
    }
}

async fn send_event_inner(tx: &EventTx, event: Value) {
    let line = serde_json::to_string(&event).unwrap();
    if tx.send(line).await.is_err() {
        eprintln!("[simplecc] stdout stalled or closed before an event was written");
    }
}

// Every call site is inside an async request/lifecycle task. Keeping the await
// in one macro makes it impossible to accidentally detach a reply while still
// leaving match arms readable.
macro_rules! send_event {
    ($tx:expr, $event:expr $(,)?) => {
        send_event_inner($tx, $event).await
    };
}

async fn primary_client(
    registry: &Arc<RwLock<Option<Registry>>>,
    language_id: &str,
) -> Option<Arc<LspClient>> {
    let registry = registry.read().await;
    registry.as_ref()?.client_for_filetype(language_id)
}

async fn primary_server_name(
    registry: &Arc<RwLock<Option<Registry>>>,
    language_id: &str,
) -> Option<String> {
    let registry = registry.read().await;
    registry.as_ref()?.primary_server_name(language_id)
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
        send_event!(
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

const USAGE: &str = "\
Usage: simplecc-daemon [OPTION]

With no arguments the daemon serves newline-delimited JSON requests on stdin
and writes replies to stdout.  That is how the Vim plugin starts it; there is
nothing useful to do with it interactively.

Options:
  -V, --version    print the version and exit
  -h, --help       print this help and exit
      --self-test  check the built-in language-server table resolves, and exit
";

/// Checks that the built-in language-server table is self-consistent.
///
/// The installer needs to know that the binary it just built actually works,
/// and a version string only proves the file is not corrupt.  Every filetype a
/// bundled server claims must resolve back to a server: when it does not, that
/// filetype silently gets no completion at all, which is the kind of failure
/// nobody reports as a bug.
fn self_test() -> Result<()> {
    let config = config::Config::default();
    if config.language_servers.is_empty() {
        anyhow::bail!("the built-in table declares no language servers");
    }

    let filetypes: std::collections::BTreeSet<String> = config
        .language_servers
        .values()
        .flat_map(|server| server.filetypes.iter().cloned())
        .collect();
    for filetype in &filetypes {
        if config.server_for_filetype(filetype).is_none() {
            anyhow::bail!("filetype '{filetype}' is claimed by a server but resolves to none");
        }
    }

    println!(
        "ok ({} servers, {} filetypes)",
        config.language_servers.len(),
        filetypes.len()
    );
    Ok(())
}

const ASYNC_SHUTDOWN_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
const RUNTIME_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("simplecc-daemon: could not start runtime: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let exit = runtime.block_on(run_cli());
    // spawn_blocking extraction/promotion has explicit size and entry bounds,
    // but a wedged filesystem must still not defeat the process-level shutdown
    // deadline after its async owner has been aborted.
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_GRACE);
    exit
}

async fn run_cli() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => match serve().await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("simplecc-daemon: {error}");
                std::process::ExitCode::FAILURE
            }
        },
        Some("--version" | "-V") => {
            println!("simplecc-daemon {}", env!("CARGO_PKG_VERSION"));
            std::process::ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("simplecc-daemon {}\n\n{USAGE}", env!("CARGO_PKG_VERSION"));
            std::process::ExitCode::SUCCESS
        }
        Some("--self-test") => match self_test() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("self-test failed: {error}");
                std::process::ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("unknown argument: {other}\n\n{USAGE}");
            std::process::ExitCode::from(2)
        }
    }
}

async fn drain_install_tasks(
    tasks: &mut tokio::task::JoinSet<()>,
    deadline: tokio::time::Instant,
) -> bool {
    let drained = tokio::time::timeout_at(deadline, async {
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                eprintln!("[simplecc] install task failed during drain: {error}");
            }
        }
    })
    .await
    .is_ok();
    if !drained {
        tasks.abort_all();
        // Give cancellation one scheduler turn so cheap async owners release
        // process/staging guards, but never start a second unbounded join.
        tokio::task::yield_now().await;
        while tasks.try_join_next().is_some() {}
    }
    drained
}

async fn serve() -> Result<()> {
    let mut stdin = BufReader::new(tokio::io::stdin());

    eprintln!("[simplecc] daemon started");

    let (sender, out_rx) = tokio::sync::mpsc::channel::<String>(4096);
    let out_tx = EventTx::new(sender);
    let (stdout_done_tx, stdout_done_rx) = std::sync::mpsc::channel();
    let stdout_thread = std::thread::spawn(move || {
        stdout_writer(out_rx);
        let _ = stdout_done_tx.send(());
    });

    let registry: Arc<RwLock<Option<Registry>>> = Arc::new(RwLock::new(None));
    let workspace_watcher: Arc<Mutex<Option<WorkspaceWatcher>>> = Arc::new(Mutex::new(None));
    // Track which filetype a URI belongs to
    let uri_ft: Arc<Mutex<std::collections::HashMap<String, String>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let mut request_tasks = tokio::task::JoinSet::new();
    // Managed-server installation is global, not tied to one LSP registry.
    // Reinitializing a workspace must not abort extraction/child processes and
    // strand their staging directory or the in-process installation marker.
    let mut install_tasks = tokio::task::JoinSet::new();

    loop {
        if out_tx.is_stalled() {
            break;
        }
        let next_line = tokio::select! {
            _ = out_tx.wait_stalled() => break,
            line = read_request_line(&mut stdin, MAX_REQUEST_LINE_BYTES) => line?,
        };
        let Some(line) = next_line else {
            break;
        };
        let line = match line {
            Ok(line) => line,
            Err(message) => {
                eprintln!("[simplecc] bad request: {message}");
                send_event!(
                    &out_tx,
                    json!({"type": "error", "id": 0, "message": message}),
                );
                continue;
            }
        };
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
        while let Some(result) = install_tasks.try_join_next() {
            if let Err(error) = result {
                eprintln!("[simplecc] install task failed: {error}");
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
            let stalled_out = out.clone();
            let stalled = tokio::select! {
                _ = stalled_out.wait_stalled() => true,
                _ = handle_request(req, reg, out, uft, watcher) => false,
            };
            if stalled {
                break;
            }
        } else if req.preserves_document_order() {
            let stalled_out = out.clone();
            let stalled = tokio::select! {
                _ = stalled_out.wait_stalled() => true,
                _ = handle_request(req, reg, out, uft, watcher) => false,
            };
            if stalled {
                break;
            }
        } else if req.is_install() {
            install_tasks.spawn(async move {
                handle_request(req, reg, out, uft, watcher).await;
            });
        } else {
            request_tasks.spawn(async move {
                handle_request(req, reg, out, uft, watcher).await;
            });
        }
    }

    let shutdown_deadline = tokio::time::Instant::now() + ASYNC_SHUTDOWN_BUDGET;

    // EOF means no more requests, not that replies to requests already
    // accepted may be discarded. Give feature tasks a bounded opportunity to
    // finish before tearing down the language servers they use; a wedged LSP
    // must still not keep a piped daemon alive forever.
    let drained = tokio::time::timeout_at(shutdown_deadline, async {
        while let Some(result) = request_tasks.join_next().await {
            if let Err(error) = result {
                eprintln!("[simplecc] request task failed during drain: {error}");
            }
        }
    })
    .await;
    if drained.is_err() {
        request_tasks.abort_all();
        tokio::task::yield_now().await;
        while request_tasks.try_join_next().is_some() {}
    }
    // Installs survive workspace lifecycle barriers, but not process EOF for
    // minutes. Cancellation never deletes a possibly live hidden staging tree;
    // Unix child groups are killed, and promotion is one non-cancellable sync
    // critical section, so async owners can be aborted at the shared deadline.
    drain_install_tasks(&mut install_tasks, shutdown_deadline).await;
    let shutdown_services = async {
        workspace_watcher.lock().await.take();
        let mut registry_to_shutdown = registry.write().await.take();
        if let Some(ref mut reg) = registry_to_shutdown {
            reg.shutdown_all().await;
        }
        drop(registry_to_shutdown);
    };
    if tokio::time::timeout_at(shutdown_deadline, shutdown_services)
        .await
        .is_err()
    {
        eprintln!("[simplecc] service shutdown exceeded the process deadline");
    }

    // The stdout writer owns the actual pipe. Let it drain every queued reply
    // (especially the shutdown acknowledgement) before the Tokio runtime tears
    // down spawned tasks at process exit.
    drop(out_tx);
    let remaining = shutdown_deadline.saturating_duration_since(tokio::time::Instant::now());
    if stdout_done_rx.recv_timeout(remaining).is_ok() {
        let _ = stdout_thread.join();
    }

    eprintln!("[simplecc] daemon exiting");
    Ok(())
}

/// Run one managed-server install and publish its terminal result.
///
/// Deliberately owns no Registry handle: an install may outlive an LSP
/// initialize/shutdown barrier, so completing it must not rewrite whichever
/// workspace registry happens to have replaced the one that requested it.
async fn handle_install_request(id: u64, server: String, out: EventTx) {
    match installer::install_server(&server, &out).await {
        Ok(path) => send_event!(
            &out,
            json!({
                "type": "installResult",
                "id": id,
                "server": server,
                "status": "ok",
                "path": path.to_string_lossy(),
            }),
        ),
        Err(error) => {
            eprintln!("[simplecc] install {server} failed: {error}");
            send_event!(
                &out,
                json!({
                    "type": "installResult",
                    "id": id,
                    "server": server,
                    "status": "error",
                    "message": error.to_string(),
                }),
            );
        }
    }
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
            remote,
            remote_config,
            python_path,
            python_lsp_path,
        } => {
            // Configuration discovery walks the filesystem; keep it off the
            // async workers.
            let load_result = tokio::task::spawn_blocking({
                let root = root.clone();
                let config_path = config_path.clone();
                move || match remote_config.as_deref() {
                    Some(content) => config::Config::parse(content),
                    None => config::Config::load_selected(&root, config_path.as_deref()),
                }
            })
            .await
            .unwrap_or_else(|error| {
                Err(anyhow::anyhow!("configuration loader task failed: {error}"))
            });
            let mut cfg = match load_result {
                Ok(config) => config,
                Err(error) => {
                    send_event!(
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

            cfg.apply_python_environment(&python_path, &python_lsp_path);

            // A successful reinitialization replaces one complete workspace;
            // stop its watcher and servers before publishing the new registry.
            workspace_watcher.lock().await.take();
            let mut registry_to_shutdown = registry.write().await.take();
            if let Some(ref mut registry) = registry_to_shutdown {
                registry.shutdown_all().await;
            }
            uri_ft.lock().await.clear();

            let reg = Registry::new(cfg, root.clone(), remote.clone(), out.clone());
            *registry.write().await = Some(reg);

            if remote.is_none() {
                match WorkspaceWatcher::start(&root, registry.clone()) {
                    Ok(watcher) => {
                        *workspace_watcher.lock().await = Some(watcher);
                        eprintln!("[simplecc] watching workspace: {root}");
                    }
                    Err(err) => {
                        eprintln!("[simplecc] workspace watcher unavailable: {err}");
                    }
                }
            } else {
                eprintln!("[simplecc] remote workspace: {root}");
            }

            send_event!(&out, json!({"type": "initialized", "id": id}));
        }

        Request::Shutdown { id } => {
            workspace_watcher.lock().await.take();
            let mut registry_to_shutdown = registry.write().await.take();
            if let Some(ref mut reg) = registry_to_shutdown {
                reg.shutdown_all().await;
            }
            uri_ft.lock().await.clear();
            send_event!(&out, json!({"type": "shutdown", "id": id}));
        }

        Request::JuliaActivateEnvironment {
            id,
            language_id,
            env_path,
        } => {
            if language_id != "julia" {
                send_event!(
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
                        if let Some(watcher) = workspace_watcher.lock().await.as_mut()
                            && let Err(err) = watcher.watch_julia_environment(&env_path)
                        {
                            eprintln!("[simplecc] {err}");
                        }
                        send_event!(
                            &out,
                            json!({
                                "type": "juliaEnvironment",
                                "id": id,
                                "path": env_path,
                            }),
                        );
                    }
                    Err(err) => send_event!(
                        &out,
                        json!({"type": "error", "id": id, "message": err.to_string()}),
                    ),
                },
                None => send_event!(
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
                send_event!(
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
                    Ok(true) => send_event!(&out, json!({"type": "juliaRefreshed", "id": id})),
                    Ok(false) => send_event!(
                        &out,
                        json!({
                            "type": "error",
                            "id": id,
                            "message": "Active server is not Julia LanguageServer",
                        }),
                    ),
                    Err(err) => send_event!(
                        &out,
                        json!({"type": "error", "id": id, "message": err.to_string()}),
                    ),
                },
                None => send_event!(
                    &out,
                    json!({
                        "type": "error",
                        "id": id,
                        "message": "Julia language server is not running",
                    }),
                ),
            }
        }

        Request::ReloadConfiguration {
            id,
            config_path,
            remote_config,
        } => {
            let result = {
                let mut registry = registry.write().await;
                match registry.as_mut() {
                    Some(registry) => {
                        registry
                            .reload_configuration(config_path.as_deref(), remote_config.as_deref())
                            .await
                    }
                    None => Err(anyhow::anyhow!("SimpleCC is not initialized")),
                }
            };

            match result {
                Ok(server_count) => send_event!(
                    &out,
                    json!({
                        "type": "configurationReloaded",
                        "id": id,
                        "servers": server_count,
                    }),
                ),
                Err(err) => send_event!(
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
                    match registry.ensure_server(&language_id, &uri).await {
                        Ok(Some(_name)) => registry.clients_for_filetype(&language_id),
                        _ => Vec::new(),
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

        Request::DidChangeWatchedFiles { id: _, changes } => {
            let changes: Vec<(String, u32)> = changes
                .into_iter()
                .map(|change| (change.uri, change.change_type))
                .collect();
            if changes.is_empty() {
                return;
            }
            // Routing is decided by each server's dynamic registration
            // (did_change_watched_files filters through it), not by any
            // filetype, so every running client is offered the batch.
            let clients = registry
                .read()
                .await
                .as_ref()
                .map(Registry::active_clients)
                .unwrap_or_default();
            for client in clients {
                let _ = client.did_change_watched_files(&changes).await;
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
            sort_items,
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
                        id,
                        &uri,
                        line,
                        character,
                        max_items,
                        trigger_kind,
                        trigger_character,
                        sort_items,
                    )
                    .await
                {
                    Ok(Some((generation, items))) => send_event!(
                        &out,
                        json!({
                            "type": "completion", "id": id,
                            "generation": generation, "items": items
                        }),
                    ),
                    Ok(None) => {}
                    Err(e) => send_event!(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            } else {
                send_event!(
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
            debug_log!(
                "[simplecc] hover request: uri={} lang={} line={} char={}",
                uri,
                language_id,
                line,
                character
            );

            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.hover(id, &uri, line, character).await {
                    Ok(Some(Some(contents))) => {
                        debug_log!("[simplecc] hover result: {} bytes", contents.len());
                        send_event!(
                            &out,
                            json!({"type": "hover", "id": id, "contents": contents}),
                        );
                    }
                    Ok(Some(None)) => {
                        debug_log!("[simplecc] hover result: none");
                        send_event!(&out, json!({"type": "hover", "id": id, "contents": null}));
                    }
                    // Superseded by a newer hover; that reply follows.
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("[simplecc] hover error: {}", e);
                        send_event!(
                            &out,
                            json!({"type": "error", "id": id, "message": e.to_string()}),
                        );
                    }
                }
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
            debug_log!(
                "[simplecc] definition request: uri={} lang={} line={} char={}",
                uri,
                language_id,
                line,
                character
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
                                        debug_log!(
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
                        debug_log!("[simplecc] definition result: {} locations", locs.len());
                        send_event!(
                            &out,
                            json!({"type": "definition", "id": id, "locations": locs}),
                        );
                    }
                    Err(e) => {
                        eprintln!("[simplecc] definition error: {}", e);
                        send_event!(
                            &out,
                            json!({"type": "error", "id": id, "message": e.to_string()}),
                        );
                    }
                }
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
                    Ok(locs) => send_event!(
                        &out,
                        json!({"type": "references", "id": id, "locations": locs}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(actions) => send_event!(
                        &out,
                        json!({"type": "codeAction", "id": id, "actions": actions}),
                    ),
                    Err(e) => send_event!(
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
                        send_event!(&out, json!({"type": "applyEdit", "id": id, "edit": edit}))
                    }
                    Ok(None) => send_event!(&out, json!({"type": "executeAction", "id": id})),
                    Err(e) => send_event!(
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
                    Ok(edits) => send_event!(
                        &out,
                        json!({"type": "formatting", "id": id, "edits": edits}),
                    ),
                    Err(e) => send_event!(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::RangeFormatting {
            id,
            uri,
            language_id,
            line,
            character,
            end_line,
            end_character,
            tab_size,
            insert_spaces,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c
                    .range_formatting(
                        &uri,
                        line,
                        character,
                        end_line,
                        end_character,
                        tab_size,
                        insert_spaces,
                    )
                    .await
                {
                    // Same reply type as whole-document formatting: the editor
                    // applies the edits identically either way.
                    Ok(edits) => send_event!(
                        &out,
                        json!({"type": "formatting", "id": id, "edits": edits}),
                    ),
                    Err(e) => send_event!(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::PrepareRename {
            id,
            uri,
            language_id,
            line,
            character,
        } => {
            if let Some(client) = primary_client_or_error(&registry, &out, id, &language_id).await {
                let c = client;
                match c.prepare_rename(&uri, line, character).await {
                    Ok(Some(item)) => send_event!(
                        &out,
                        json!({"type": "prepareRename", "id": id, "result": item}),
                    ),
                    Ok(None) => send_event!(
                        &out,
                        json!({"type": "prepareRename", "id": id, "result": null}),
                    ),
                    Err(e) => send_event!(
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
                        send_event!(&out, json!({"type": "rename", "id": id, "edit": edit}))
                    }
                    Ok(None) => {
                        send_event!(&out, json!({"type": "rename", "id": id, "edit": null}))
                    }
                    Err(e) => send_event!(
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
                match c.signature_help(id, &uri, line, character).await {
                    Ok(Some(sigs)) if !sigs.is_empty() => send_event!(
                        &out,
                        json!({"type": "signatureHelp", "id": id, "signatures": sigs}),
                    ),
                    Ok(Some(_)) => send_event!(
                        &out,
                        json!({"type": "signatureHelp", "id": id, "signatures": null}),
                    ),
                    // Superseded by a newer request; skip so a stale null can
                    // never close the popup the newest reply just opened.
                    Ok(None) => {}
                    Err(e) => send_event!(
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
                    Ok(locs) => send_event!(
                        &out,
                        json!({"type": "implementation", "id": id, "locations": locs}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(locs) => send_event!(
                        &out,
                        json!({"type": "typeDefinition", "id": id, "locations": locs}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(symbols) => send_event!(
                        &out,
                        json!({"type": "documentSymbol", "id": id, "symbols": symbols}),
                    ),
                    Err(e) => send_event!(
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
                match c.workspace_symbol(id, &query).await {
                    Ok(Some(symbols)) => send_event!(
                        &out,
                        json!({"type": "workspaceSymbol", "id": id, "symbols": symbols}),
                    ),
                    // Superseded by a newer query; the newer reply follows.
                    Ok(None) => {}
                    Err(e) => send_event!(
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
                match c.document_highlight(id, &uri, line, character).await {
                    Ok(Some(highlights)) => send_event!(
                        &out,
                        json!({"type": "documentHighlight", "id": id, "highlights": highlights}),
                    ),
                    // Superseded by a newer cursor position; skip the reply so
                    // stale results never overwrite the upcoming ones.
                    Ok(None) => {}
                    Err(e) => send_event!(
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
                match c.inlay_hints(id, &uri, start_line, end_line).await {
                    Ok(Some(hints)) => {
                        send_event!(&out, json!({"type": "inlayHint", "id": id, "hints": hints}))
                    }
                    // Superseded by a newer viewport; skip the stale reply.
                    Ok(None) => {}
                    Err(e) => send_event!(
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
                        send_event!(
                            &out,
                            json!({"type": "callHierarchyPrepare", "id": id, "items": converted}),
                        );
                    }
                    Err(e) => send_event!(
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
                        send_event!(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid call hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.call_hierarchy_incoming(&lsp_item).await {
                    Ok(calls) => send_event!(
                        &out,
                        json!({"type": "incomingCalls", "id": id, "calls": calls}),
                    ),
                    Err(e) => send_event!(
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
                        send_event!(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid call hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.call_hierarchy_outgoing(&lsp_item).await {
                    Ok(calls) => send_event!(
                        &out,
                        json!({"type": "outgoingCalls", "id": id, "calls": calls}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(ranges) => send_event!(
                        &out,
                        json!({"type": "selectionRange", "id": id, "ranges": ranges}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(tokens) => send_event!(
                        &out,
                        json!({"type": "semanticTokens", "id": id, "tokens": tokens}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(tokens) => send_event!(
                        &out,
                        json!({"type": "semanticTokens", "id": id, "tokens": tokens}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(tokens) => send_event!(
                        &out,
                        json!({"type": "semanticTokens", "id": id, "tokens": tokens}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(lenses) => send_event!(
                        &out,
                        json!({"type": "codeLens", "id": id, "lenses": lenses}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(ranges) => send_event!(
                        &out,
                        json!({"type": "foldingRange", "id": id, "ranges": ranges}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(Some(ranges)) => send_event!(
                        &out,
                        json!({"type": "linkedEditingRange", "id": id, "result": ranges}),
                    ),
                    Ok(None) => send_event!(
                        &out,
                        json!({"type": "linkedEditingRange", "id": id, "result": null}),
                    ),
                    Err(e) => send_event!(
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
                    Ok(item) => send_event!(
                        &out,
                        json!({"type": "completionResolve", "id": id, "item": item}),
                    ),
                    Err(e) => send_event!(
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
                        send_event!(&out, json!({"type": "applyEdit", "id": id, "edit": edit}))
                    }
                    Ok(None) => send_event!(&out, json!({"type": "codeLensExecute", "id": id})),
                    Err(e) => send_event!(
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
                        send_event!(
                            &out,
                            json!({"type": "typeHierarchyPrepare", "id": id, "items": converted}),
                        );
                    }
                    Err(e) => send_event!(
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
                        send_event!(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid type hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.type_hierarchy_supertypes(&lsp_item).await {
                    Ok(items) => send_event!(
                        &out,
                        json!({"type": "supertypes", "id": id, "items": items}),
                    ),
                    Err(e) => send_event!(
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
                        send_event!(
                            &out,
                            json!({"type": "error", "id": id, "message": format!("invalid type hierarchy item: {error}")}),
                        );
                        return;
                    }
                };
                match c.type_hierarchy_subtypes(&lsp_item).await {
                    Ok(items) => {
                        send_event!(&out, json!({"type": "subtypes", "id": id, "items": items}))
                    }
                    Err(e) => send_event!(
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
                // Same publisher as this server's pushed diagnostics, so a
                // file that gets both does not end up listed twice.
                let server = primary_server_name(&registry, &language_id)
                    .await
                    .unwrap_or_default();
                match c.pull_diagnostics(&uri).await {
                    Ok(Some(items)) => send_event!(
                        &out,
                        json!({
                            "type": "diagnostics", "id": id, "server": server,
                            "uri": uri, "items": items
                        }),
                    ),
                    // An unchanged report keeps the currently displayed set.
                    Ok(None) => {}
                    Err(e) => send_event!(
                        &out,
                        json!({"type": "error", "id": id, "message": e.to_string()}),
                    ),
                }
            }
        }

        Request::InstallServer { id, server } => handle_install_request(id, server, out).await,

        Request::ListInstallable { id } => {
            let servers = installer::list_installable();
            send_event!(
                &out,
                json!({
                    "type": "installableServers",
                    "id": id,
                    "servers": servers,
                }),
            );
        }

        Request::ServerResponse {
            id: _,
            server,
            request_id,
            result,
        } => {
            let client = {
                let registry = registry.read().await;
                registry
                    .as_ref()
                    .and_then(|registry| registry.client_by_name(&server))
            };
            match client {
                Some(client) => {
                    if let Err(error) = client.respond_to_server(request_id, result).await {
                        eprintln!("[simplecc] failed to answer {server} request: {error}");
                    }
                }
                None => {
                    eprintln!("[simplecc] server response for unknown server: {server}");
                }
            }
        }
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[tokio::test]
    async fn bounded_request_reader_recovers_at_the_next_record() {
        let input = b"0123456789\n{\"type\":\"shutdown\",\"id\":7}\r\n";
        let mut reader = BufReader::new(&input[..]);

        let oversized = read_request_line(&mut reader, 8)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(oversized, "request line exceeds 8 bytes");

        let next = read_request_line(&mut reader, 64)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(next, r#"{"type":"shutdown","id":7}"#);
        assert!(read_request_line(&mut reader, 64).await.unwrap().is_none());

        let mut exact_crlf = BufReader::new(&b"12345678\r\n"[..]);
        assert_eq!(
            read_request_line(&mut exact_crlf, 8)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            "12345678"
        );
    }

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
            remote: None,
            remote_config: None,
            python_path: String::new(),
            python_lsp_path: String::new(),
        };
        let shutdown = Request::Shutdown { id: 2 };

        assert!(initialize.preserves_document_order());
        assert!(initialize.is_lifecycle_barrier());
        assert!(shutdown.preserves_document_order());
        assert!(shutdown.is_lifecycle_barrier());
    }

    #[test]
    fn managed_install_is_not_an_lsp_lifecycle_task() {
        let install = Request::InstallServer {
            id: 3,
            server: "rust-analyzer".to_string(),
        };
        assert!(install.is_install());
        assert!(!install.is_lifecycle_barrier());
        assert!(!install.preserves_document_order());
    }

    #[tokio::test]
    async fn install_completion_is_decoupled_from_the_current_registry() {
        // The install helper deliberately has no Registry argument. An install
        // can span a workspace reinitialize, and its eventual result must not
        // overwrite the replacement registry's local or remote command.
        let (sender, mut rx) = tokio::sync::mpsc::channel(4);
        let tx = EventTx::new(sender);
        handle_install_request(17, "not-a-simplecc-server".to_string(), tx).await;

        let event: serde_json::Value =
            serde_json::from_str(&rx.recv().await.expect("one terminal install result")).unwrap();
        assert_eq!(event["type"], "installResult");
        assert_eq!(event["id"], 17);
        assert_eq!(event["server"], "not-a-simplecc-server");
        assert_eq!(event["status"], "error");
    }

    #[tokio::test]
    async fn eof_install_drain_aborts_a_hung_async_owner_after_its_grace() {
        struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }

        let mut tasks = tokio::task::JoinSet::new();
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task_dropped = dropped.clone();
        tasks.spawn(async move {
            let _drop_flag = DropFlag(task_dropped);
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
        });
        ready_rx.await.unwrap();

        let drained = drain_install_tasks(
            &mut tasks,
            tokio::time::Instant::now() + std::time::Duration::from_millis(20),
        )
        .await;
        assert!(
            !drained,
            "a permanently pending install reported completion"
        );
        assert!(tasks.is_empty());
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn replies_wait_for_capacity_instead_of_being_dropped() {
        let (sender, mut rx) = tokio::sync::mpsc::channel(1);
        let tx = EventTx::new(sender);
        tx.send("already queued".to_string()).await.unwrap();

        let producer = tokio::spawn(async move {
            send_event!(&tx, json!({"type": "reply", "id": 42}));
        });

        assert_eq!(rx.recv().await.as_deref(), Some("already queued"));
        let reply = rx.recv().await.unwrap();
        producer.await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["id"], 42);
    }

    #[tokio::test]
    async fn missing_primary_client_returns_error_with_request_id() {
        let registry = Arc::new(RwLock::new(None));
        let (sender, mut rx) = tokio::sync::mpsc::channel(1);
        let tx = EventTx::new(sender);

        let client = primary_client_or_error(&registry, &tx, 73, "rust").await;

        assert!(client.is_none());
        let reply: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(reply["type"], "error");
        assert_eq!(reply["id"], 73);
        assert!(
            reply["message"]
                .as_str()
                .unwrap()
                .contains("no active language server")
        );
    }

    #[test]
    fn parses_prepare_rename_requests() {
        let request: Request = serde_json::from_value(json!({
            "type": "textDocument/prepareRename",
            "id": 3,
            "uri": "file:///tmp/main.rs",
            "languageId": "rust",
            "line": 1,
            "character": 2
        }))
        .unwrap();

        assert!(matches!(request, Request::PrepareRename { id: 3, .. }));
    }

    #[test]
    fn parses_server_responses_and_keeps_them_ordered() {
        let request: Request = serde_json::from_value(json!({
            "type": "server/response",
            "server": "rust-analyzer",
            "requestId": 7,
            "result": { "applied": true }
        }))
        .unwrap();

        match &request {
            Request::ServerResponse {
                server, request_id, ..
            } => {
                assert_eq!(server, "rust-analyzer");
                assert_eq!(request_id, &json!(7));
            }
            _ => panic!("unexpected request variant"),
        }
        // A waiting language server must never queue behind feature tasks.
        assert!(request.preserves_document_order());
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
            Request::ReloadConfiguration {
                id,
                config_path,
                remote_config,
            } => {
                assert_eq!(id, 9);
                assert_eq!(config_path.as_deref(), Some("/tmp/simplecc.json"));
                assert!(remote_config.is_none());
            }
            _ => panic!("unexpected request variant"),
        }
    }

    #[test]
    fn parses_configuration_reload_with_remote_config() {
        let request: Request = serde_json::from_value(json!({
            "type": "workspace/reloadConfiguration",
            "id": 10,
            "configPath": "",
            "remoteConfig": "{\"languageServers\":{}}"
        }))
        .unwrap();

        match request {
            Request::ReloadConfiguration {
                id,
                config_path,
                remote_config,
            } => {
                assert_eq!(id, 10);
                assert_eq!(config_path.as_deref(), Some(""));
                assert_eq!(remote_config.as_deref(), Some("{\"languageServers\":{}}"));
            }
            _ => panic!("unexpected request variant"),
        }
    }

    #[test]
    fn parses_watched_file_changes_and_keeps_them_ordered() {
        let request: Request = serde_json::from_value(json!({
            "type": "workspace/didChangeWatchedFiles",
            "id": 12,
            "changes": [
                {"uri": "file:///srv/app/new.py", "type": 1},
                {"uri": "file:///srv/app/old.py", "type": 3},
                {"uri": "file:///srv/app/touched.py"}
            ]
        }))
        .unwrap();

        match &request {
            Request::DidChangeWatchedFiles { id, changes } => {
                assert_eq!(*id, 12);
                let seen: Vec<(&str, u32)> = changes
                    .iter()
                    .map(|change| (change.uri.as_str(), change.change_type))
                    .collect();
                assert_eq!(
                    seen,
                    vec![
                        ("file:///srv/app/new.py", 1),
                        ("file:///srv/app/old.py", 3),
                        // A missing type is a plain change.
                        ("file:///srv/app/touched.py", 2),
                    ]
                );
            }
            _ => panic!("unexpected request variant"),
        }
        // A rename arrives as deleted+created and must follow the
        // didClose/didOpen of the buffers it moved.
        assert!(request.preserves_document_order());
        assert!(!request.is_lifecycle_barrier());
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
