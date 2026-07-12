from pathlib import Path
import re

PATH = Path("src/simplecc/simplecc_daemon.rs")
text = PATH.read_text(encoding="utf-8")


def replace_exact(old: str, new: str, expected: int = 1) -> None:
    global text
    count = text.count(old)
    if count != expected:
        raise RuntimeError(
            f"expected {expected} exact matches, found {count}: {old[:120]!r}"
        )
    text = text.replace(old, new)


def replace_regex(pattern: str, replacement, expected: int) -> None:
    global text
    text, count = re.subn(pattern, replacement, text, flags=re.S)
    if count != expected:
        raise RuntimeError(
            f"expected {expected} regex matches, found {count}: {pattern[:120]!r}"
        )


def matching_brace(source: str, open_index: int) -> int:
    if source[open_index] != "{":
        raise RuntimeError("matching_brace called without an opening brace")
    depth = 0
    i = open_index
    in_string = False
    in_line_comment = False
    block_comment_depth = 0
    escaped = False
    while i < len(source):
        ch = source[i]
        nxt = source[i + 1] if i + 1 < len(source) else ""
        if in_line_comment:
            if ch == "\n":
                in_line_comment = False
            i += 1
            continue
        if block_comment_depth:
            if ch == "/" and nxt == "*":
                block_comment_depth += 1
                i += 2
                continue
            if ch == "*" and nxt == "/":
                block_comment_depth -= 1
                i += 2
                continue
            i += 1
            continue
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            i += 1
            continue
        if ch == "/" and nxt == "/":
            in_line_comment = True
            i += 2
            continue
        if ch == "/" and nxt == "*":
            block_comment_depth = 1
            i += 2
            continue
        if ch == '"':
            in_string = True
            i += 1
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise RuntimeError("unterminated Rust brace block")


def collapse_two_level_blocks(prefix: str, replacement_open: str, minimum: int) -> int:
    global text
    starts = []
    cursor = 0
    while True:
        index = text.find(prefix, cursor)
        if index < 0:
            break
        starts.append(index)
        cursor = index + len(prefix)
    if len(starts) < minimum:
        raise RuntimeError(
            f"expected at least {minimum} guarded blocks, found {len(starts)}"
        )

    first_open_offset = prefix.find("{")
    second_open_offset = prefix.find("{", first_open_offset + 1)
    if first_open_offset < 0 or second_open_offset < 0:
        raise RuntimeError("guard prefix must contain two opening braces")

    for start in reversed(starts):
        outer_open = start + first_open_offset
        inner_open = start + second_open_offset
        outer_close = matching_brace(text, outer_open)
        inner_close = matching_brace(text, inner_open)
        if not inner_close < outer_close:
            raise RuntimeError("unexpected guarded block nesting")
        if text[inner_close + 1 : outer_close].strip():
            raise RuntimeError("unexpected content between nested guard closings")
        body = text[start + len(prefix) : inner_close]
        text = text[:start] + replacement_open + body + "}" + text[outer_close + 1 :]
    return len(starts)


# Import the concrete client type used by the short-lived snapshot helpers.
replace_exact(
    "use lsp::types;\n",
    "use lsp::client::LspClient;\nuse lsp::types;\n",
)

# Snapshot helpers clone Arc<LspClient> values while the read guard is held and
# release the registry before any language-server await begins.
replace_exact(
    '''fn send_event(tx: &EventTx, event: Value) {
    let s = serde_json::to_string(&event).unwrap();
    let _ = tx.try_send(s);
}
''',
    '''fn send_event(tx: &EventTx, event: Value) {
    let s = serde_json::to_string(&event).unwrap();
    let _ = tx.try_send(s);
}

async fn primary_client(
    registry: &Arc<RwLock<Option<Registry>>>,
    language_id: &str,
) -> Option<Arc<LspClient>> {
    let registry = registry.read().await;
    registry.as_ref()?.client_for_filetype(language_id)
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
''',
)

# Shutdown takes ownership of the Registry first, releasing the global write
# guard before waiting for individual language servers to exit.
shutdown_pattern = r'''(?P<indent>^[ ]*)let mut r = registry\.write\(\)\.await;\n(?P=indent)if let Some\(ref mut reg\) = \*r \{\n(?P=indent)    reg\.shutdown_all\(\)\.await;\n(?P=indent)\}'''

def shutdown_replacement(match: re.Match[str]) -> str:
    indent = match.group("indent")
    return (
        f"{indent}let mut registry_to_shutdown = registry.write().await.take();\n"
        f"{indent}if let Some(ref mut reg) = registry_to_shutdown {{\n"
        f"{indent}    reg.shutdown_all().await;\n"
        f"{indent}}}"
    )

replace_regex(shutdown_pattern, shutdown_replacement, expected=2)

# didOpen may need an exclusive guard while starting a server, but the guard is
# released before didOpen notifications are fanned out to the clients.
replace_exact(
    '''            let mut r = registry.write().await;
            if let Some(ref mut reg) = *r {
                // Ensure server started for this filetype
                if let Ok(Some(_name)) = reg.ensure_server(&language_id).await {
                    // Fan out to all servers for this filetype
                    let clients = reg.clients_for_filetype(&language_id);
                    for client in clients {
                        let c = client;
                        let _ = c.did_open(&uri, &language_id, version, &text).await;
                    }
                }
            }
''',
    '''            let clients = {
                let mut registry = registry.write().await;
                if let Some(ref mut registry) = *registry {
                    if let Ok(Some(_name)) = registry.ensure_server(&language_id).await {
                        registry.clients_for_filetype(&language_id)
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            };
            for client in clients {
                let _ = client
                    .did_open(&uri, &language_id, version, &text)
                    .await;
            }
''',
)

# didChange/didSave/didClose clone the client list and release the registry
# before sending notifications.
fanout_prefix = '''            let r = registry.read().await;
            if let Some(ref reg) = *r {
                for client in reg.clients_for_filetype(&ft) {
                    let c = client;'''
fanout_count = collapse_two_level_blocks(
    fanout_prefix,
    "            for c in filetype_clients(&registry, &ft).await {",
    minimum=3,
)

# All ordinary single-client features now acquire an Arc snapshot through the
# helper, so hover/definition/semantic tokens/etc. never pin a Registry guard.
primary_prefix = '''            let r = registry.read().await;
            if let Some(ref reg) = *r {
                if let Some(client) = reg.client_for_filetype(&language_id) {
                    let c = client;'''
primary_count = collapse_two_level_blocks(
    primary_prefix,
    "            if let Some(c) = primary_client(&registry, &language_id).await {",
    minimum=20,
)

# The completion and completion-resolve handlers already use a short snapshot
# block. Keep those explicit blocks, but reject every old long-lived template.
assert fanout_prefix not in text
assert primary_prefix not in text
assert "let c = client;" not in text
assert "registry.write().await.take()" in text
assert "async fn primary_client(" in text
assert "async fn filetype_clients(" in text
assert text.count("registry_to_shutdown") == 4

PATH.write_text(text, encoding="utf-8")
print(
    "released registry guards:",
    f"{primary_count} primary feature blocks,",
    f"{fanout_count} document fanout blocks",
)
