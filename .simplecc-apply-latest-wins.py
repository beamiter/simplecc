from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    file_path = Path(path)
    text = file_path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise RuntimeError(
            f"{path}: expected one exact match, found {count}: {old[:120]!r}"
        )
    file_path.write_text(text.replace(old, new, 1), encoding="utf-8")


def regex_once(path: str, pattern: str, replacement: str) -> None:
    file_path = Path(path)
    text = file_path.read_text(encoding="utf-8")
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(
            f"{path}: expected one regex match, found {count}: {pattern[:120]!r}"
        )
    file_path.write_text(updated, encoding="utf-8")


CLIENT = "src/simplecc/lsp/client.rs"
DAEMON = "src/simplecc/simplecc_daemon.rs"

# Track the newest cancellable request per logical feature key.
replace_once(
    CLIENT,
    '''    /// Pending requests: jsonrpc id -> oneshot sender
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    /// Server capabilities after initialize
''',
    '''    /// Pending requests: jsonrpc id -> oneshot sender
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    /// Newest in-flight request id per latest-wins feature key.
    latest_requests: Arc<Mutex<HashMap<String, i64>>>,
    /// Server capabilities after initialize
''',
)
replace_once(
    CLIENT,
    '''            pending,
            capabilities,
''',
    '''            pending,
            latest_requests: Arc::new(Mutex::new(HashMap::new())),
            capabilities,
''',
)

# Replace the request lifecycle with a shared implementation that supports
# latest-wins cancellation without logging superseded requests as errors.
regex_once(
    CLIENT,
    r'''    /// Send a JSON-RPC request with a feature-specific timeout\. Timed-out\n.*?\n    /// Send a JSON-RPC notification \(no response expected\)\.''',
    '''    /// Send a JSON-RPC request with a feature-specific timeout. Timed-out
    /// requests are removed from the pending map and cancelled at the server,
    /// preventing leaked senders and very late responses from accumulating.
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        match self
            .request_with_timeout_inner(None, method, params, timeout)
            .await?
        {
            Some(result) => Ok(result),
            None => bail!("non-superseding request was unexpectedly cancelled: {method}"),
        }
    }

    /// Send a request where only the newest request for `key` is useful.
    /// Starting a replacement drops the previous response channel and emits
    /// `$/cancelRequest`. Superseded calls return `Ok(None)` without producing
    /// a daemon error event.
    async fn request_latest_with_timeout(
        &self,
        key: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Option<Value>> {
        self.request_with_timeout_inner(Some(key), method, params, timeout)
            .await
    }

    async fn send_message(&self, msg: &Value) -> Result<()> {
        let mut transport = self.transport.lock().await;
        transport.send(msg).await
    }

    async fn cancel_pending_request(&self, id: i64) {
        let removed = self.pending.lock().await.remove(&id).is_some();
        if !removed {
            return;
        }

        let cancel = json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": id },
        });
        let _ = self.send_message(&cancel).await;
    }

    /// Remove `key` only if it still points at `id`. A false return means a
    /// newer request replaced this one while it was waiting for the server.
    async fn clear_latest_request(&self, key: &str, id: i64) -> bool {
        let mut latest = self.latest_requests.lock().await;
        if latest.get(key).copied() != Some(id) {
            return false;
        }
        latest.remove(key);
        true
    }

    async fn request_with_timeout_inner(
        &self,
        latest_key: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Option<Value>> {
        let id = self.next_request_id();
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        // Hold the latest-request map while cancelling the previous request and
        // writing the replacement. This guarantees the server observes
        // request(old) -> cancel(old) -> request(new), never cancel-before-send.
        let send_result = if let Some(key) = latest_key {
            let mut latest = self.latest_requests.lock().await;
            if let Some(previous_id) = latest.insert(key.to_string(), id) {
                self.cancel_pending_request(previous_id).await;
            }
            let result = self.send_message(&msg).await;
            if result.is_err() && latest.get(key).copied() == Some(id) {
                latest.remove(key);
            }
            result
        } else {
            self.send_message(&msg).await
        };

        if let Err(err) = send_result {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }

        let resp = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(err)) => {
                self.pending.lock().await.remove(&id);
                if let Some(key) = latest_key {
                    if !self.clear_latest_request(key, id).await {
                        return Ok(None);
                    }
                }
                return Err(err.into());
            }
            Err(_) => {
                self.cancel_pending_request(id).await;
                if let Some(key) = latest_key {
                    self.clear_latest_request(key, id).await;
                }
                bail!(
                    "LSP request timed out after {}ms: {}",
                    timeout.as_millis(),
                    method,
                );
            }
        };

        if let Some(key) = latest_key {
            if !self.clear_latest_request(key, id).await {
                return Ok(None);
            }
        }

        if let Some(err) = resp.get("error") {
            bail!("LSP error: {}", err);
        }

        Ok(Some(
            resp.get("result").cloned().unwrap_or(Value::Null),
        ))
    }

    /// Send a JSON-RPC notification (no response expected).''',
)

# Reuse the send helper for notifications, keeping transport serialization in
# one place.
replace_once(
    CLIENT,
    '''        let mut t = self.transport.lock().await;
        t.send(&msg).await
''',
    '''        self.send_message(&msg).await
''',
)

# Completion is the hottest latest-wins path. A superseded result is suppressed
# before parsing or touching the completion resolve cache.
replace_once(
    CLIENT,
    '''    ) -> Result<(u64, Vec<types::CompletionItem>)> {
''',
    '''    ) -> Result<Option<(u64, Vec<types::CompletionItem>)>> {
''',
)
replace_once(
    CLIENT,
    '''        let result = self
            .request_with_timeout(
                "textDocument/completion",
''',
    '''        let request_key = format!("completion:{uri}");
        let result = match self
            .request_latest_with_timeout(
                &request_key,
                "textDocument/completion",
''',
)
replace_once(
    CLIENT,
    '''            )
            .await?;

        let mut items = if result.is_array() {
''',
    '''            )
            .await?
        {
            Some(result) => result,
            None => return Ok(None),
        };

        let mut items = if result.is_array() {
''',
)
replace_once(
    CLIENT,
    '''        Ok((generation, normalized))
''',
    '''        Ok(Some((generation, normalized)))
''',
)

# The daemon emits no event for a superseded request. Vim already tracks the
# newest editor request id, so only the replacement request needs a response.
replace_once(
    DAEMON,
    '''                    Ok((generation, items)) => send_event(
''',
    '''                    Ok(Some((generation, items))) => send_event(
''',
)
replace_once(
    DAEMON,
    '''                    ),
                    Err(e) => send_event(
''',
    '''                    ),
                    Ok(None) => {}
                    Err(e) => send_event(
''',
)

client_text = Path(CLIENT).read_text(encoding="utf-8")
daemon_text = Path(DAEMON).read_text(encoding="utf-8")
assert "latest_requests: Arc<Mutex<HashMap<String, i64>>>" in client_text
assert "request_latest_with_timeout" in client_text
assert 'format!("completion:{uri}")' in client_text
assert "Ok(None) => {}" in daemon_text
assert "Result<Option<(u64, Vec<types::CompletionItem>)>>" in client_text

print("applied latest-wins completion cancellation")
