use anyhow::{Context, Result};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::registry::Registry;

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

const BATCH_IDLE: Duration = Duration::from_millis(120);
const BATCH_MAX_AGE: Duration = Duration::from_millis(500);
const BATCH_MAX_EVENTS: usize = 1024;

// Cap both elapsed coalescing time and work per batch. The input queue remains
// lossless: stopping a batch leaves later events for the next notification.
// This is not a queue-memory bound or a bound on slow language-server writes.
async fn collect_batch(
    first: Event,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<notify::Result<Event>>,
) -> HashMap<PathBuf, u32> {
    let deadline = tokio::time::Instant::now() + BATCH_MAX_AGE;
    let mut pending = HashMap::new();
    merge_event(&mut pending, first);
    for _ in 1..BATCH_MAX_EVENTS {
        let now = tokio::time::Instant::now();
        // A ready receiver may win a timeout even after its deadline. Check
        // explicitly so a continuously nonempty queue cannot starve delivery.
        if now >= deadline {
            break;
        }
        let wait = BATCH_IDLE.min(deadline - now);
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(Ok(event))) => merge_event(&mut pending, event),
            Ok(Some(Err(err))) => eprintln!("[simplecc] workspace watcher error: {err}"),
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
}

impl WorkspaceWatcher {
    pub fn start(root: &str, registry: Arc<RwLock<Option<Registry>>>) -> Result<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .context("failed to create platform file watcher")?;
        let root_path = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        watcher
            .watch(&root_path, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch workspace {root}"))?;

        let task = tokio::spawn(async move {
            let mut last_julia_refresh: Option<Instant> = None;
            while let Some(result) = rx.recv().await {
                let first = match result {
                    Ok(event) => event,
                    Err(err) => {
                        eprintln!("[simplecc] workspace watcher error: {err}");
                        continue;
                    }
                };

                // Coalesce save bursts without waiting forever for a busy
                // workspace to become quiet.
                let pending = collect_batch(first, &mut rx).await;

                let manifest_changed = pending.keys().any(|path| is_julia_manifest(path));
                // Each language server supplies its own dynamic watcher globs.
                // Forward the complete coalesced batch and let the client-side
                // registration matcher select the events for that server.
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
                    && last_julia_refresh
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(2));
                let mut refreshed_julia = false;
                for client in clients {
                    if let Err(err) = client.did_change_watched_files(&changes).await {
                        eprintln!("[simplecc] watched-file notification failed: {err}");
                    }
                    if should_refresh_julia {
                        match client.refresh_julia_language_server().await {
                            Ok(true) => refreshed_julia = true,
                            Ok(false) => {}
                            Err(err) => {
                                eprintln!("[simplecc] Julia symbol-cache refresh failed: {err}")
                            }
                        }
                    }
                }
                if refreshed_julia {
                    last_julia_refresh = Some(Instant::now());
                }
            }
        });

        Ok(Self {
            watcher,
            task,
            root: root_path,
            julia_environment: None,
        })
    }

    /// Follow the selected Julia environment even when it lives outside the
    /// workspace (for example ~/.julia/environments/v1.x).
    pub fn watch_julia_environment(&mut self, environment: &str) -> Result<()> {
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

    #[tokio::test]
    async fn continuous_events_do_not_wait_for_quiet() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let producer = tokio::spawn(async move {
            loop {
                if tx.send(Ok(change("busy.rs"))).is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            collect_batch(change("first.rs"), &mut rx),
        )
        .await;
        producer.abort();
        let pending = result.expect("continuous events must not starve dispatch");
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn backlog_is_split_without_dropping_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for i in 1..=BATCH_MAX_EVENTS {
            tx.send(Ok(change(&format!("{i}.rs")))).unwrap();
        }
        drop(tx);
        let first = collect_batch(change("0.rs"), &mut rx).await;
        assert_eq!(first.len(), BATCH_MAX_EVENTS);
        let remaining = rx.recv().await.unwrap().unwrap();
        let second = collect_batch(remaining, &mut rx).await;
        assert_eq!(second.len(), 1);
        assert!(second.contains_key(&PathBuf::from(format!("{BATCH_MAX_EVENTS}.rs"))));
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn closed_batch_preserves_save_and_rename_semantics() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(Err(notify::Error::generic("test error"))).unwrap();
        tx.send(Ok(
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path("temp.rs".into())
                .add_path("saved.rs".into()),
        ))
        .unwrap();
        tx.send(Ok(change("saved.rs"))).unwrap();
        drop(tx);
        let pending = collect_batch(change("temp.rs"), &mut rx).await;
        assert_eq!(pending[Path::new("temp.rs")], DELETED);
        assert_eq!(pending[Path::new("saved.rs")], CREATED);
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
