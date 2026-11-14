use anyhow::{Context, Result};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::registry::{EventTx, Registry};

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

const BATCH_IDLE: Duration = Duration::from_millis(120);
const BATCH_MAX_AGE: Duration = Duration::from_millis(500);
const BATCH_MAX_EVENTS: usize = 1024;

// Burst headroom, not a license to grow indefinitely behind a slow server.
const INPUT_MAX_EVENTS: usize = 16_384;
const INPUT_MAX_PATH_BYTES: usize = 16 * 1024 * 1024;
const EVENT_MAX_PATH_BYTES: usize = 64 * 1024;
const EVENT_MAX_PATHS: usize = 256;

#[derive(Default)]
struct InputState {
    failed: AtomicBool,
    failure: tokio::sync::Notify,
    path_bytes: AtomicUsize,
}

impl InputState {
    fn fail(&self) {
        if !self.failed.swap(true, Ordering::AcqRel) {
            // Store a permit even when the reporter has not started polling.
            self.failure.notify_one();
        }
    }

    fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    fn reserve_paths(&self, bytes: usize) -> bool {
        let mut used = self.path_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = used
                .checked_add(bytes)
                .filter(|&n| n <= INPUT_MAX_PATH_BYTES)
            else {
                return false;
            };
            match self.path_bytes.compare_exchange_weak(
                used,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => used = current,
            }
        }
    }

    async fn wait_failed(&self) {
        loop {
            let notified = self.failure.notified();
            if self.is_failed() {
                return;
            }
            notified.await;
        }
    }
}

struct QueuedEvent {
    event: Option<Event>,
    bytes: usize,
    state: Arc<InputState>,
}

impl QueuedEvent {
    fn take(mut self) -> Event {
        self.event.take().expect("queued event is consumed once")
    }
}

impl Drop for QueuedEvent {
    fn drop(&mut self) {
        self.state
            .path_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Clone)]
struct EventInput {
    tx: tokio::sync::mpsc::Sender<QueuedEvent>,
    state: Arc<InputState>,
}

impl EventInput {
    fn new(capacity: usize) -> (Self, tokio::sync::mpsc::Receiver<QueuedEvent>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);
        (
            Self {
                tx,
                state: Arc::default(),
            },
            rx,
        )
    }

    // Runs on the notify backend thread: never wait for a language server or
    // queue capacity. A separate signal cannot be crowded out of a full queue.
    fn push(&self, result: notify::Result<Event>) {
        if self.state.is_failed() || self.tx.is_closed() {
            return;
        }
        let event = match result {
            Ok(event) if !event.need_rescan() => event,
            Ok(_) => {
                self.state.fail();
                return;
            }
            Err(_) => {
                // No synchronous stderr I/O on the backend callback: even a
                // blocked log pipe must not delay the independent reporter.
                self.state.fail();
                return;
            }
        };
        if matches!(
            event.kind,
            EventKind::Access(_) | EventKind::Other | EventKind::Any
        ) {
            return;
        }
        if event.paths.is_empty() {
            return;
        }
        let bytes = event.paths.iter().try_fold(0usize, |sum, path| {
            sum.checked_add(path.as_os_str().as_encoded_bytes().len())
        });
        let Some(bytes) = bytes.filter(|&n| n <= EVENT_MAX_PATH_BYTES) else {
            self.state.fail();
            return;
        };
        if event.paths.len() > EVENT_MAX_PATHS || !self.state.reserve_paths(bytes) {
            self.state.fail();
            return;
        }
        // Retain only the data consumed below, not arbitrary backend metadata
        // or oversized spare Vec/PathBuf capacity. Rescan was handled above.
        let mut clean = Event::new(event.kind);
        clean.paths = event
            .paths
            .into_iter()
            .map(|path| path.into_boxed_path().into_path_buf())
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .into_vec();
        let queued = QueuedEvent {
            event: Some(clean),
            bytes,
            state: self.state.clone(),
        };
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = self.tx.try_send(queued) {
            self.state.fail();
        }
    }
}

// Bound coalescing time and work as well as the upstream queue. Budget charges
// are released on dequeue; this is an input-queue bound, not whole-daemon RSS.
async fn collect_batch(
    first: Event,
    rx: &mut tokio::sync::mpsc::Receiver<QueuedEvent>,
    state: &InputState,
) -> HashMap<PathBuf, u32> {
    let deadline = tokio::time::Instant::now() + BATCH_MAX_AGE;
    let mut pending = HashMap::new();
    merge_event(&mut pending, first);
    for _ in 1..BATCH_MAX_EVENTS {
        let now = tokio::time::Instant::now();
        if now >= deadline || state.is_failed() {
            break;
        }
        let wait = BATCH_IDLE.min(deadline - now);
        // The independent reporter owns the failure Notify; only one waiter
        // consumes its stored permit. Polling here is bounded by BATCH_IDLE.
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(event)) => merge_event(&mut pending, event.take()),
            Ok(None) | Err(_) => break,
        }
    }
    pending
}

/// Keeps the platform watcher and its async event-forwarding task alive for
/// the lifetime of one initialized workspace.
pub struct WorkspaceWatcher {
    watcher: RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
    root: PathBuf,
    julia_environment: Option<PathBuf>,
    input_state: Arc<InputState>,
}

impl WorkspaceWatcher {
    pub fn start(
        root: &str,
        registry: Arc<RwLock<Option<Registry>>>,
        out: EventTx,
    ) -> Result<Self> {
        let (input, rx) = EventInput::new(INPUT_MAX_EVENTS);
        let input_state = input.state.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            input.push(event);
        })
        .context("failed to create platform file watcher")?;
        let root_path = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        watcher
            .watch(&root_path, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch workspace {root}"))?;

        let task = tokio::spawn(forward_events(rx, registry, out, input_state.clone()));

        Ok(Self {
            watcher,
            task,
            root: root_path,
            julia_environment: None,
            input_state,
        })
    }

    /// Follow the selected Julia environment even when it lives outside the
    /// workspace (for example ~/.julia/environments/v1.x).
    pub fn watch_julia_environment(&mut self, environment: &str) -> Result<()> {
        if self.input_state.is_failed() {
            anyhow::bail!("workspace watcher is disabled; run :SimpleCCRestart");
        }
        let environment =
            std::fs::canonicalize(environment).unwrap_or_else(|_| PathBuf::from(environment));
        if self.julia_environment.as_ref() == Some(&environment) {
            return Ok(());
        }

        if let Some(previous) = self.julia_environment.take()
            && !previous.starts_with(&self.root)
        {
            let _ = self.watcher.unwatch(&previous);
        }

        if !environment.starts_with(&self.root) {
            self.watcher
                .watch(&environment, RecursiveMode::NonRecursive)
                .with_context(|| {
                    format!(
                        "failed to watch Julia environment {}",
                        environment.display()
                    )
                })?;
        }
        self.julia_environment = Some(environment);
        Ok(())
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn forward_events(
    rx: tokio::sync::mpsc::Receiver<QueuedEvent>,
    registry: Arc<RwLock<Option<Registry>>>,
    out: EventTx,
    state: Arc<InputState>,
) {
    supervise_forwarding(forward_batches(rx, registry, &state), &out, &state).await;
}

async fn supervise_forwarding(
    forward: impl std::future::Future<Output = ()>,
    out: &EventTx,
    state: &InputState,
) {
    // Reporting must not queue behind a language server that stopped reading.
    // Do not cancel an in-flight framed LSP write, which could corrupt stdio.
    let (finished, done) = tokio::sync::oneshot::channel();
    let forward = async {
        forward.await;
        let _ = finished.send(());
    };
    let reporter = async {
        tokio::select! {
            biased;
            _ = state.wait_failed() => {},
            _ = done => {},
        }
        if state.is_failed() {
            let _ = report_event_loss(out, &mut false).await;
        }
    };
    tokio::join!(forward, reporter);
}

async fn forward_batches(
    mut rx: tokio::sync::mpsc::Receiver<QueuedEvent>,
    registry: Arc<RwLock<Option<Registry>>>,
    state: &InputState,
) {
    let mut last_julia_refresh: Option<Instant> = None;
    while !state.is_failed() {
        // The reporter does not consume queued input. Wake even if a pathless
        // rescan/error arrives while this receiver is empty.
        let first = match tokio::time::timeout(BATCH_IDLE, rx.recv()).await {
            Ok(Some(event)) => event.take(),
            Ok(None) => break,
            Err(_) => continue,
        };
        let pending = collect_batch(first, &mut rx, state).await;
        if state.is_failed() {
            break;
        }
        let manifest_changed = pending.keys().any(|path| is_julia_manifest(path));
        let changes = workspace_file_changes(pending);
        if changes.is_empty() {
            continue;
        }
        let clients = registry
            .read()
            .await
            .as_ref()
            .map(Registry::active_clients)
            .unwrap_or_default();
        let should_refresh_julia = manifest_changed
            && last_julia_refresh.is_none_or(|last| last.elapsed() >= Duration::from_secs(2));
        let mut refreshed_julia = false;
        for client in clients {
            if state.is_failed() {
                break;
            }
            if let Err(err) = client.did_change_watched_files(&changes).await {
                eprintln!("[simplecc] watched-file notification failed: {err}");
            }
            if should_refresh_julia && !state.is_failed() {
                match client.refresh_julia_language_server().await {
                    Ok(true) => refreshed_julia = true,
                    Ok(false) => {}
                    Err(err) => eprintln!("[simplecc] Julia symbol-cache refresh failed: {err}"),
                }
            }
        }
        if refreshed_julia {
            last_julia_refresh = Some(Instant::now());
        }
    }
    // Dropping rx releases all pending input and budget reservations. The
    // backend's callback remains nonblocking and inert until explicit restart.
}

// Preserve delivery through the same bounded, fail-closed path as other daemon
// errors. A full Vim output queue must not silently hide the loss notification.
async fn report_event_loss(out: &EventTx, reported: &mut bool) -> Result<()> {
    if !*reported {
        out.send(serde_json::json!({
            "type": "showMessage",
            "level": "error",
            "message": "SimpleCC workspace watcher disabled after event loss, backend error, or input limit. Further watcher notifications are stopped; an in-flight write may finish. File state may be stale. Automatic rescan is not implemented. Run :SimpleCCRestart to reinitialize servers and watching before relying on diagnostics."
        }).to_string()).await.map_err(|_| anyhow::anyhow!("failed to report watcher event loss"))?;
        *reported = true;
    }
    Ok(())
}

fn merge_event(pending: &mut HashMap<PathBuf, u32>, event: Event) {
    match event.kind {
        EventKind::Create(_) => merge_paths(pending, event.paths, CREATED),
        EventKind::Remove(_) => merge_paths(pending, event.paths, DELETED),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() >= 2 => {
            merge_path(pending, event.paths[0].clone(), DELETED);
            merge_path(pending, event.paths[1].clone(), CREATED);
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            merge_paths(pending, event.paths, DELETED)
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            merge_paths(pending, event.paths, CREATED)
        }
        EventKind::Modify(_) => merge_paths(pending, event.paths, CHANGED),
        EventKind::Access(_) | EventKind::Other | EventKind::Any => {}
    }
}

fn merge_paths(pending: &mut HashMap<PathBuf, u32>, paths: Vec<PathBuf>, change_type: u32) {
    for path in paths {
        merge_path(pending, path, change_type);
    }
}

fn merge_path(pending: &mut HashMap<PathBuf, u32>, path: PathBuf, change_type: u32) {
    pending
        .entry(path)
        .and_modify(|existing| {
            // Preserve a final delete/create over preceding write noise.
            if change_type != CHANGED || *existing == CHANGED {
                *existing = change_type;
            }
        })
        .or_insert(change_type);
}

fn workspace_file_changes(pending: HashMap<PathBuf, u32>) -> Vec<(String, u32)> {
    let mut changes: Vec<_> = pending
        .into_iter()
        .filter_map(|(path, change_type)| {
            url::Url::from_file_path(path)
                .ok()
                .map(|uri| (uri.to_string(), change_type))
        })
        .collect();
    changes.sort_unstable();
    changes
}

fn is_julia_manifest(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(name, "Manifest.toml" | "JuliaManifest.toml")
        || ((name.starts_with("Manifest-v") || name.starts_with("JuliaManifest-v"))
            && name.ends_with(".toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};

    fn change(path: &str) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.into())
    }

    fn rescan() -> Event {
        // Both Linux IN_Q_OVERFLOW and macOS MUST_SCAN_SUBDIRS can arrive
        // without paths and with EventKind::Other.
        Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
    }

    #[tokio::test]
    async fn input_overflow_is_visible_and_latched() {
        let (input, rx) = EventInput::new(8);
        let state = input.state.clone();
        for index in 0..100_000 {
            input.push(Ok(change(&format!("/tmp/{index}.rs"))));
        }
        assert!(state.is_failed());
        assert_eq!(rx.len(), 8);
        drop(input);
        let (sender, mut output) = tokio::sync::mpsc::channel(4);
        tokio::time::timeout(
            Duration::from_secs(2),
            forward_events(
                rx,
                Arc::new(RwLock::new(None)),
                EventTx::new(sender),
                state.clone(),
            ),
        )
        .await
        .unwrap();
        let message: serde_json::Value = serde_json::from_str(&output.try_recv().unwrap()).unwrap();
        assert_eq!(message["type"], "showMessage");
        assert_eq!(message["level"], "error");
        assert!(
            message["message"]
                .as_str()
                .unwrap()
                .contains(":SimpleCCRestart")
        );
        assert!(message["message"].as_str().unwrap().contains("disabled"));
        assert!(output.try_recv().is_err());
        assert_eq!(state.path_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn pathless_rescan_and_error_wake_empty_receiver() {
        for result in [Ok(rescan()), Err(notify::Error::generic("backend failure"))] {
            let (input, rx) = EventInput::new(8);
            let state = input.state.clone();
            let (sender, mut output) = tokio::sync::mpsc::channel(4);
            let task = tokio::spawn(forward_events(
                rx,
                Arc::new(RwLock::new(None)),
                EventTx::new(sender),
                state,
            ));
            tokio::task::yield_now().await;
            input.push(result);
            let message = tokio::time::timeout(Duration::from_secs(1), output.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(message.contains("Automatic rescan is not implemented"));
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            assert!(input.tx.is_closed());
        }
    }

    #[tokio::test]
    async fn warning_does_not_cancel_or_wait_for_blocked_dispatcher() {
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let state = Arc::new(InputState::default());
        let failure = state.clone();
        let (sender, mut output) = tokio::sync::mpsc::channel(4);
        let task = tokio::spawn(async move {
            supervise_forwarding(
                async {
                    entered.send(()).unwrap();
                    blocked.await.unwrap();
                },
                &EventTx::new(sender),
                &state,
            )
            .await;
        });
        started.await.unwrap();
        failure.fail();
        let message = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(message.contains("disabled"));
        assert!(!task.is_finished(), "do not cancel an in-flight dispatcher");
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn aggregate_path_budget_is_released_on_consume_and_drop() {
        let (input, mut rx) = EventInput::new(INPUT_MAX_EVENTS);
        let long = "x".repeat(EVENT_MAX_PATH_BYTES);
        for _ in 0..(INPUT_MAX_PATH_BYTES / EVENT_MAX_PATH_BYTES) {
            input.push(Ok(change(&long)));
        }
        assert!(!input.state.is_failed());
        assert_eq!(
            input.state.path_bytes.load(Ordering::Acquire),
            INPUT_MAX_PATH_BYTES
        );
        drop(rx.try_recv().unwrap().take());
        assert_eq!(
            input.state.path_bytes.load(Ordering::Acquire),
            INPUT_MAX_PATH_BYTES - EVENT_MAX_PATH_BYTES
        );
        input.push(Ok(change(&long)));
        assert!(!input.state.is_failed());
        input.push(Ok(change("overflow.rs")));
        assert!(input.state.is_failed());
        assert_eq!(
            input.state.path_bytes.load(Ordering::Acquire),
            INPUT_MAX_PATH_BYTES
        );
        drop(rx);
        assert_eq!(input.state.path_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn oversized_event_is_not_retained() {
        for event in [
            change(&"x".repeat(EVENT_MAX_PATH_BYTES + 1)),
            (0..=EVENT_MAX_PATHS).fold(Event::new(EventKind::Modify(ModifyKind::Any)), |e, i| {
                e.add_path(format!("{i}.rs").into())
            }),
        ] {
            let (input, rx) = EventInput::new(8);
            input.push(Ok(event));
            assert!(input.state.is_failed());
            assert_eq!(rx.len(), 0);
            assert_eq!(input.state.path_bytes.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn concurrent_producers_cannot_exceed_path_budget() {
        let (input, rx) = EventInput::new(INPUT_MAX_EVENTS);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let input = input.clone();
                scope.spawn(move || {
                    let path = "x".repeat(EVENT_MAX_PATH_BYTES);
                    for _ in 0..128 {
                        input.push(Ok(change(&path)));
                    }
                });
            }
        });
        assert!(input.state.is_failed());
        assert!(input.state.path_bytes.load(Ordering::Acquire) <= INPUT_MAX_PATH_BYTES);
        assert!(rx.len() <= INPUT_MAX_PATH_BYTES / EVENT_MAX_PATH_BYTES);
        drop(rx);
        assert_eq!(input.state.path_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn ignored_noise_does_not_consume_burst_capacity() {
        let (input, rx) = EventInput::new(8);
        for _ in 0..100_000 {
            input.push(Ok(Event::new(EventKind::Access(
                notify::event::AccessKind::Any,
            ))));
            input.push(Ok(Event::new(EventKind::Other)));
            input.push(Ok(Event::new(EventKind::Any)));
        }
        assert!(!input.state.is_failed());
        assert_eq!(rx.len(), 0);
    }

    #[tokio::test]
    async fn burst_headroom_preserves_every_path_across_batches() {
        let (input, mut rx) = EventInput::new(INPUT_MAX_EVENTS);
        let state = input.state.clone();
        for index in 0..4_096 {
            input.push(Ok(change(&format!("/tmp/{index}.rs"))));
        }
        assert!(!state.is_failed());
        drop(input);
        let mut all = HashMap::new();
        while let Some(first) = rx.recv().await {
            let batch = collect_batch(first.take(), &mut rx, &state).await;
            assert!(batch.len() <= BATCH_MAX_EVENTS);
            all.extend(batch);
        }
        assert_eq!(all.len(), 4_096);
        assert_eq!(state.path_bytes.load(Ordering::Acquire), 0);
        assert!(!state.is_failed());
    }

    #[tokio::test]
    async fn continuous_events_do_not_wait_for_quiet() {
        let (input, mut rx) = EventInput::new(INPUT_MAX_EVENTS);
        let state = input.state.clone();
        let producer = tokio::spawn(async move {
            loop {
                input.push(Ok(change("busy.rs")));
                if input.tx.is_closed() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let pending = tokio::time::timeout(
            Duration::from_secs(2),
            collect_batch(change("first.rs"), &mut rx, &state),
        )
        .await
        .unwrap();
        producer.abort();
        assert_eq!(pending.len(), 2);
        assert!(!state.is_failed());
    }

    #[tokio::test]
    async fn closed_batch_preserves_save_and_rename_semantics() {
        let (input, mut rx) = EventInput::new(8);
        let state = input.state.clone();
        input.push(Ok(Event::new(EventKind::Modify(ModifyKind::Name(
            RenameMode::Both,
        )))
        .add_path("temp.rs".into())
        .add_path("saved.rs".into())));
        input.push(Ok(change("saved.rs")));
        drop(input);
        let pending = collect_batch(change("temp.rs"), &mut rx, &state).await;
        assert_eq!(pending[Path::new("temp.rs")], DELETED);
        assert_eq!(pending[Path::new("saved.rs")], CREATED);
        assert!(!state.is_failed());
    }

    #[tokio::test]
    async fn warning_waits_for_output_capacity_without_silent_drop() {
        let (input, rx) = EventInput::new(1);
        input.push(Ok(rescan()));
        let (sender, mut output) = tokio::sync::mpsc::channel(1);
        sender.send("already queued".into()).await.unwrap();
        let out = EventTx::new(sender);
        let task = tokio::spawn(forward_events(
            rx,
            Arc::new(RwLock::new(None)),
            out.clone(),
            input.state.clone(),
        ));
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        assert_eq!(output.recv().await.unwrap(), "already queued");
        let error = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(error.contains(":SimpleCCRestart"));
        task.await.unwrap();
        assert!(!out.is_stalled());
    }

    #[tokio::test]
    async fn failed_warning_delivery_is_not_acknowledged() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let out = EventTx::new(sender);
        let mut reported = false;
        assert!(report_event_loss(&out, &mut reported).await.is_err());
        assert!(!reported);
        assert!(out.is_stalled());
    }

    #[tokio::test]
    async fn recreated_watcher_delivers_real_create_and_delete_to_lsp() {
        use serde_json::{Value, json};
        let root = std::env::temp_dir().join(format!(
            "simplecc-watcher-restart-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        // macOS temp paths may be aliases under /private; match the watcher's
        // canonical root for both dynamic registration and expected URIs.
        let root = std::fs::canonicalize(root).unwrap();
        let server = r#"
import json, sys
changes, waiting = [], []
registered = False
def send(value):
    body = json.dumps(dict(jsonrpc='2.0', **value)).encode()
    sys.stdout.buffer.write(b'Content-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
    sys.stdout.buffer.flush()
while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line in (b'\r\n', b'\n'): break
        key, value = line.decode().split(':', 1)
        headers[key.lower()] = value.strip()
    msg = json.loads(sys.stdin.buffer.read(int(headers['content-length'])))
    method = msg.get('method')
    if method == 'initialize':
        send(dict(id=msg['id'], result={'capabilities': {}}))
    elif method == 'initialized':
        send(dict(id=999, method='client/registerCapability', params={'registrations': [{
            'id': 'files', 'method': 'workspace/didChangeWatchedFiles',
            'registerOptions': {'watchers': [{'globPattern': '**/*.rs', 'kind': 7}]}
        }]}))
    elif method == 'test/ready':
        if registered: send(dict(id=msg['id'], result=True))
        else: waiting.append(msg['id'])
    elif method is None and msg.get('id') == 999:
        registered = True
        for id in waiting: send(dict(id=id, result=True))
        waiting = []
    elif method == 'workspace/didChangeWatchedFiles':
        changes.extend(msg['params']['changes'])
    elif method == 'test/changes':
        send(dict(id=msg['id'], result=changes))
    elif method == 'shutdown':
        send(dict(id=msg['id'], result=None))
    elif method == 'exit': break
"#;
        let config = serde_json::from_value(json!({"languageServers": {"watcher-test": {
            "command": "python3", "args": ["-u", "-c", server], "filetypes": ["testwatch"]
        }}}))
        .unwrap();
        let (sender, mut output) = tokio::sync::mpsc::channel(64);
        let out = EventTx::new(sender);
        let mut reg = Registry::new(
            config,
            root.to_string_lossy().into_owned(),
            None,
            out.clone(),
        );
        let uri = url::Url::from_file_path(root.join("restart.rs"))
            .unwrap()
            .to_string();
        reg.ensure_server("testwatch", &uri).await.unwrap().unwrap();
        let client = reg.client_for_filetype("testwatch").unwrap();
        assert_eq!(
            client.request("test/ready", Value::Null).await.unwrap(),
            json!(true)
        );
        let registry = Arc::new(RwLock::new(Some(reg)));
        let mut old =
            WorkspaceWatcher::start(root.to_str().unwrap(), registry.clone(), out.clone()).unwrap();
        old.input_state.fail();
        assert!(old.watch_julia_environment(root.to_str().unwrap()).is_err());
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if output.recv().await.unwrap().contains(":SimpleCCRestart") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(old);
        let mut fresh =
            WorkspaceWatcher::start(root.to_str().unwrap(), registry.clone(), out).unwrap();
        fresh
            .watch_julia_environment(root.to_str().unwrap())
            .unwrap();
        let file = root.join("restart.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        for expected in [CREATED, DELETED] {
            if expected == DELETED {
                std::fs::remove_file(&file).unwrap();
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let changes = client.request("test/changes", Value::Null).await.unwrap();
                    if changes
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["uri"] == uri && c["type"] == expected)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("recreated watcher must deliver actual file lifecycle events to LSP");
        }
        assert!(!fresh.input_state.is_failed());
        drop(fresh);
        registry
            .write()
            .await
            .as_mut()
            .unwrap()
            .shutdown_all()
            .await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forwards_all_workspace_extensions_for_per_server_glob_matching() {
        let pending = HashMap::from([
            (PathBuf::from("/tmp/simplecc/src/main.rs"), CREATED),
            (PathBuf::from("/tmp/simplecc/lib/tool.py"), CHANGED),
            (PathBuf::from("/tmp/simplecc/src/main.jl"), DELETED),
            (PathBuf::from("/tmp/simplecc/target/cache.bin"), CHANGED),
        ]);
        let changes = workspace_file_changes(pending);

        assert_eq!(changes.len(), 4);
        assert!(
            changes
                .iter()
                .any(|(uri, kind)| uri.ends_with("/src/main.rs") && *kind == CREATED)
        );
        assert!(
            changes
                .iter()
                .any(|(uri, kind)| uri.ends_with("/lib/tool.py") && *kind == CHANGED)
        );
        assert!(
            changes
                .iter()
                .any(|(uri, kind)| uri.ends_with("/src/main.jl") && *kind == DELETED)
        );
        assert!(
            changes
                .iter()
                .any(|(uri, kind)| uri.ends_with("/target/cache.bin") && *kind == CHANGED)
        );
    }

    #[test]
    fn maps_create_change_delete_and_atomic_rename() {
        let mut pending = HashMap::new();
        merge_event(
            &mut pending,
            Event::new(EventKind::Create(CreateKind::File)).add_path("new.jl".into()),
        );
        merge_event(
            &mut pending,
            Event::new(EventKind::Remove(RemoveKind::File)).add_path("old.jl".into()),
        );
        merge_event(
            &mut pending,
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path("temp.jl".into())
                .add_path("saved.jl".into()),
        );

        assert_eq!(pending[Path::new("new.jl")], CREATED);
        assert_eq!(pending[Path::new("old.jl")], DELETED);
        assert_eq!(pending[Path::new("temp.jl")], DELETED);
        assert_eq!(pending[Path::new("saved.jl")], CREATED);
    }

    #[test]
    fn recognizes_all_supported_julia_manifest_names() {
        for path in [
            "Manifest.toml",
            "JuliaManifest.toml",
            "Manifest-v1.12.toml",
            "JuliaManifest-v1.11.toml",
        ] {
            assert!(is_julia_manifest(Path::new(path)), "{path}");
        }
        for path in ["Project.toml", "Manifest.txt", "src/Manifest.jl"] {
            assert!(!is_julia_manifest(Path::new(path)), "{path}");
        }
    }
}
