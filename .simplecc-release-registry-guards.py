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
    text, count = re.subn(pattern, replacement, text, flags=re.S | re.M)
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


def unwrap_registry_read_blocks() -> tuple[int, int]:
    global text
    prefixes = [
        '''            let r = registry.read().await;
            if let Some(ref reg) = *r {''',
        '''                let r = registry.read().await;
                if let Some(ref reg) = *r {''',
    ]
    primary_token = (
        "if let Some(client) = reg.client_for_filetype(&language_id) {"
    )
    fanout_token = "for client in reg.clients_for_filetype(&ft) {"

    candidates: list[tuple[int, str]] = []
    for prefix in prefixes:
        cursor = 0
        while True:
            index = text.find(prefix, cursor)
            if index < 0:
                break
            candidates.append((index, prefix))
            cursor = index + len(prefix)

    primary_count = 0
    fanout_count = 0
    for start, prefix in sorted(candidates, reverse=True):
        outer_open = start + prefix.rfind("{")
        outer_close = matching_brace(text, outer_open)
        body = text[start + len(prefix) : outer_close]

        if primary_token in body:
            if body.count(primary_token) != 1:
                raise RuntimeError("ambiguous primary-client registry block")
            body = body.replace(
                primary_token,
                "if let Some(client) = primary_client(&registry, &language_id).await {",
                1,
            )
            primary_count += 1
        elif fanout_token in body:
            if body.count(fanout_token) != 1:
                raise RuntimeError("ambiguous filetype fan-out registry block")
            body = body.replace(
                fanout_token,
                "for client in filetype_clients(&registry, &ft).await {",
                1,
            )
            fanout_count += 1
        else:
            continue

        text = text[:start] + body + text[outer_close + 1 :]

    return primary_count, fanout_count


replace_exact(
    "use lsp::types;\n",
    "use lsp::client::LspClient;\nuse lsp::types;\n",
)

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

primary_count, fanout_count = unwrap_registry_read_blocks()
if primary_count < 20:
    raise RuntimeError(f"expected at least 20 primary client blocks, found {primary_count}")
if fanout_count != 3:
    raise RuntimeError(f"expected 3 document fan-out blocks, found {fanout_count}")

assert "reg.client_for_filetype(&language_id)" not in text
assert "reg.clients_for_filetype(&ft)" not in text
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
