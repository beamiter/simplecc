use anyhow::{Context, Result};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::registry::Registry;

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

/// Keeps the platform watcher and its async event-forwarding task alive for
/// the lifetime of one initialized workspace.
pub struct WorkspaceWatcher {
    _watcher: RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl WorkspaceWatcher {
    pub fn start(root: &str, registry: Arc<RwLock<Option<Registry>>>) -> Result<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .context("failed to create platform file watcher")?;
        watcher
            .watch(Path::new(root), RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch workspace {root}"))?;

        let task = tokio::spawn(async move {
            while let Some(result) = rx.recv().await {
                let mut pending = HashMap::<PathBuf, u32>::new();
                match result {
                    Ok(event) => merge_event(&mut pending, event),
                    Err(err) => {
                        eprintln!("[simplecc] workspace watcher error: {err}");
                        continue;
                    }
                }

                // Editors commonly produce a burst of write/rename events for
                // one logical save. Coalesce a short batch before notifying LSPs.
                loop {
                    match tokio::time::timeout(Duration::from_millis(120), rx.recv()).await {
                        Ok(Some(Ok(event))) => merge_event(&mut pending, event),
                        Ok(Some(Err(err))) => {
                            eprintln!("[simplecc] workspace watcher error: {err}")
                        }
                        Ok(None) | Err(_) => break,
                    }
                }

                let changes: Vec<_> = pending
                    .into_iter()
                    .filter_map(|(path, change_type)| {
                        is_lsp_workspace_file(&path).then(|| {
                            url::Url::from_file_path(path)
                                .ok()
                                .map(|uri| (uri.to_string(), change_type))
                        })?
                    })
                    .collect();
                if changes.is_empty() {
                    continue;
                }

                let clients = registry
                    .read()
                    .await
                    .as_ref()
                    .map(Registry::active_clients)
                    .unwrap_or_default();
                for client in clients {
                    if let Err(err) = client.did_change_watched_files(&changes).await {
                        eprintln!("[simplecc] watched-file notification failed: {err}");
                    }
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            task,
        })
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

fn is_lsp_workspace_file(path: &Path) -> bool {
    if matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("jl" | "jmd" | "md")
    ) {
        return true;
    }

    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        "Project.toml"
            | "JuliaProject.toml"
            | "Manifest.toml"
            | "JuliaManifest.toml"
            | ".JuliaLint.toml"
    ) || ((name.starts_with("Manifest-v") || name.starts_with("JuliaManifest-v"))
        && name.ends_with(".toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};

    #[test]
    fn filters_the_same_workspace_files_as_julia_language_server() {
        for path in [
            "src/main.jl",
            "notes/readme.md",
            "Project.toml",
            "Manifest-v1.12.toml",
            ".JuliaLint.toml",
        ] {
            assert!(is_lsp_workspace_file(Path::new(path)), "{path}");
        }
        for path in ["target/cache.bin", ".git/index", "data/table.csv"] {
            assert!(!is_lsp_workspace_file(Path::new(path)), "{path}");
        }
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
}
