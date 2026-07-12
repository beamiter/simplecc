from pathlib import Path


def replace_exact(path: str, old: str, new: str, expected: int = 1) -> None:
    file_path = Path(path)
    text = file_path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != expected:
        raise RuntimeError(
            f"{path}: expected {expected} matches, found {count}: {old[:100]!r}"
        )
    file_path.write_text(text.replace(old, new), encoding="utf-8")


def replace_all(path: str, old: str, new: str, minimum: int = 1) -> int:
    file_path = Path(path)
    text = file_path.read_text(encoding="utf-8")
    count = text.count(old)
    if count < minimum:
        raise RuntimeError(
            f"{path}: expected at least {minimum} matches, found {count}: {old[:100]!r}"
        )
    file_path.write_text(text.replace(old, new), encoding="utf-8")
    return count


# Registry owns immutable, internally synchronized LSP clients directly.
replace_exact("src/simplecc/registry.rs", "use tokio::sync::Mutex;\n", "")
replace_exact(
    "src/simplecc/registry.rs",
    "    clients: HashMap<String, Arc<Mutex<LspClient>>>,",
    "    clients: HashMap<String, Arc<LspClient>>,",
)
replace_exact(
    "src/simplecc/registry.rs",
    "                let client = Arc::new(Mutex::new(client));",
    "                let client = Arc::new(client);",
)
replace_exact(
    "src/simplecc/registry.rs",
    "    pub fn client_for_filetype(&self, filetype: &str) -> Option<Arc<Mutex<LspClient>>> {",
    "    pub fn client_for_filetype(&self, filetype: &str) -> Option<Arc<LspClient>> {",
)
replace_exact(
    "src/simplecc/registry.rs",
    "    pub fn clients_for_filetype(&self, filetype: &str) -> Vec<Arc<Mutex<LspClient>>> {",
    "    pub fn clients_for_filetype(&self, filetype: &str) -> Vec<Arc<LspClient>> {",
)
replace_exact(
    "src/simplecc/registry.rs",
    "    pub fn client_by_name(&self, name: &str) -> Option<Arc<Mutex<LspClient>>> {",
    "    pub fn client_by_name(&self, name: &str) -> Option<Arc<LspClient>> {",
)
replace_exact(
    "src/simplecc/registry.rs",
    "            let c = client.lock().await;\n            let _ = c.shutdown().await;",
    "            let _ = client.shutdown().await;",
)

# The registry is read far more often than it is mutated. Read guards may be
# held while feature requests await LSP responses without blocking one another.
replace_exact(
    "src/simplecc/simplecc_daemon.rs",
    "use tokio::sync::Mutex;",
    "use tokio::sync::{Mutex, RwLock};",
)
replace_exact(
    "src/simplecc/simplecc_daemon.rs",
    "    let registry: Arc<Mutex<Option<Registry>>> = Arc::new(Mutex::new(None));",
    "    let registry: Arc<RwLock<Option<Registry>>> = Arc::new(RwLock::new(None));",
)
replace_exact(
    "src/simplecc/simplecc_daemon.rs",
    "    registry: Arc<Mutex<Option<Registry>>>,",
    "    registry: Arc<RwLock<Option<Registry>>>,",
)

# Start with read locks everywhere. Sites that bind a mutable guard or replace
# the registry are converted back to write locks below.
replace_all(
    "src/simplecc/simplecc_daemon.rs",
    "registry.lock().await",
    "registry.read().await",
    minimum=5,
)
replace_all(
    "src/simplecc/simplecc_daemon.rs",
    "let mut r = registry.read().await;",
    "let mut r = registry.write().await;",
    minimum=2,
)
replace_exact(
    "src/simplecc/simplecc_daemon.rs",
    "*registry.read().await = Some(reg);",
    "*registry.write().await = Some(reg);",
)
# The background installer captures the registry under a different variable
# name and mutates server configuration, so it also needs an exclusive guard.
replace_exact(
    "src/simplecc/simplecc_daemon.rs",
    "let mut r = reg_clone.lock().await;",
    "let mut r = reg_clone.write().await;",
)

# LspClient already synchronizes transport, pending requests, capabilities and
# caches internally. Remove every outer per-client lock before an LSP await.
clone_locks = replace_all(
    "src/simplecc/simplecc_daemon.rs",
    "let c = client.lock().await.clone();",
    "let c = client;",
    minimum=2,
)
plain_locks = replace_all(
    "src/simplecc/simplecc_daemon.rs",
    "let c = client.lock().await;",
    "let c = client;",
    minimum=10,
)

# Structural assertions catch future source drift and accidental partial edits.
registry_text = Path("src/simplecc/registry.rs").read_text(encoding="utf-8")
daemon_text = Path("src/simplecc/simplecc_daemon.rs").read_text(encoding="utf-8")
assert "Arc<Mutex<LspClient>>" not in registry_text
assert "tokio::sync::Mutex" not in registry_text
assert "Arc<RwLock<Option<Registry>>>" in daemon_text
assert "registry.lock().await" not in daemon_text
assert "reg_clone.lock().await" not in daemon_text
assert "client.lock().await" not in daemon_text
assert daemon_text.count("registry.write().await") >= 3
assert daemon_text.count("registry.read().await") >= 5
assert "reg_clone.write().await" in daemon_text

print(
    "updated concurrent dispatch:",
    f"removed {clone_locks + plain_locks} outer client locks",
)
