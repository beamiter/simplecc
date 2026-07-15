use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc;

const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_CONTENT_LENGTH: usize = 64 * 1024 * 1024;
const STDERR_CHUNK_BYTES: usize = 8 * 1024;

/// LSP transport: speaks Content-Length framed JSON-RPC over stdio of a child process.
pub struct LspTransport {
    writer: Option<ChildStdin>,
    child: Child,
    terminated: bool,
}

impl LspTransport {
    /// Spawn a language server and return (transport, incoming_messages_receiver).
    pub fn spawn(
        cmd: &str,
        args: &[String],
        root_dir: Option<&str>,
    ) -> Result<(Self, mpsc::Receiver<Value>)> {
        let mut command = tokio::process::Command::new(cmd);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Explicit shutdown normally reaps the child. This is the final
            // safety net for cancellation and partially constructed clients.
            .kill_on_drop(true);
        if let Some(dir) = root_dir {
            command.current_dir(dir);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {cmd}"))?;

        let stdout = child.stdout.take().context("no stdout")?;
        let writer = child.stdin.take().context("no stdin")?;

        let (tx, rx) = mpsc::channel::<Value>(256);

        // Spawn reader task
        tokio::spawn(async move {
            if let Err(e) = read_loop(stdout, tx).await {
                eprintln!("[lsp-transport] read_loop error: {e}");
            }
        });

        // Spawn stderr drain
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                // Read bounded chunks: `read_line` can grow without limit if a
                // broken server writes an endless line to stderr.
                let mut chunk = [0u8; STDERR_CHUNK_BYTES];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => eprintln!(
                            "[lsp-stderr] {}",
                            String::from_utf8_lossy(&chunk[..read]).trim_end()
                        ),
                    }
                }
            });
        }

        Ok((
            Self {
                writer: Some(writer),
                child,
                terminated: false,
            },
            rx,
        ))
    }

    /// Send a JSON-RPC message with Content-Length header.
    pub async fn send(&mut self, msg: &Value) -> Result<()> {
        if self.terminated {
            bail!("language server transport is terminated");
        }
        let writer = self
            .writer
            .as_mut()
            .context("language server stdin is closed")?;
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        writer.write_all(header.as_bytes()).await?;
        writer.write_all(body.as_bytes()).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Close stdin and reap the language-server process. Servers get a short
    /// grace period after the LSP `exit` notification; an unresponsive process
    /// is killed so it cannot outlive the daemon or a failed initialization.
    pub async fn terminate(&mut self) -> Result<()> {
        if self.terminated {
            return Ok(());
        }

        // Closing stdin also lets a server whose stdout already disappeared
        // observe the disconnect and exit without being killed.
        self.writer.take();

        match tokio::time::timeout(std::time::Duration::from_secs(2), self.child.wait()).await {
            Ok(status) => {
                status.context("failed to reap language server")?;
            }
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .context("failed to kill unresponsive language server")?;
            }
        }
        self.terminated = true;
        Ok(())
    }
}

/// Read Content-Length framed messages from stdout.
async fn read_loop<R>(stdout: R, tx: mpsc::Sender<Value>) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stdout);
    let mut header_buf = Vec::with_capacity(256);

    loop {
        // Read headers
        let mut content_length = None;
        let mut header_bytes = 0usize;
        loop {
            let n = read_limited_header_line(&mut reader, &mut header_buf).await?;
            if n == 0 {
                return Ok(()); // EOF
            }
            header_bytes = header_bytes
                .checked_add(n)
                .context("LSP header byte count overflow")?;
            if header_bytes > MAX_HEADER_BYTES {
                bail!("LSP headers exceed {MAX_HEADER_BYTES} bytes");
            }

            let line = std::str::from_utf8(&header_buf)
                .context("LSP header is not UTF-8")?
                .trim();
            if line.is_empty() {
                break; // End of headers
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("Content-Length") {
                    if content_length.is_some() {
                        bail!("duplicate Content-Length header");
                    }
                    let length: usize = value.trim().parse().context("bad Content-Length")?;
                    if length > MAX_CONTENT_LENGTH {
                        bail!("Content-Length {length} exceeds {MAX_CONTENT_LENGTH} bytes");
                    }
                    content_length = Some(length);
                }
            }
        }

        let content_length = content_length.context("missing Content-Length")?;

        // Read body
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await?;

        let msg: Value = serde_json::from_slice(&body)?;

        if tx.send(msg).await.is_err() {
            break; // receiver dropped
        }
    }
    Ok(())
}

/// Read one header line without allowing `read_line` to grow a `String`
/// without bound when a broken or hostile server omits the newline.
async fn read_limited_header_line<R>(reader: &mut R, line: &mut Vec<u8>) -> Result<usize>
where
    R: AsyncBufRead + Unpin,
{
    line.clear();
    loop {
        let (consumed, has_newline) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(line.len());
            }

            let consumed = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |position| position + 1);
            if line.len().saturating_add(consumed) > MAX_HEADER_LINE_BYTES {
                bail!("LSP header line exceeds {MAX_HEADER_LINE_BYTES} bytes");
            }
            let has_newline = available[consumed - 1] == b'\n';
            line.extend_from_slice(&available[..consumed]);
            (consumed, has_newline)
        };
        reader.consume(consumed);

        if has_newline {
            return Ok(line.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_loop_accepts_case_insensitive_content_length() {
        let body = br#"{"jsonrpc":"2.0","method":"test"}"#;
        let (mut writer, reader) = tokio::io::duplex(1024);
        writer
            .write_all(format!("content-length: {}\r\n\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        writer.write_all(body).await.unwrap();
        drop(writer);

        let (tx, mut rx) = mpsc::channel(1);
        read_loop(reader, tx).await.unwrap();

        assert_eq!(rx.recv().await.unwrap()["method"], "test");
    }

    #[tokio::test]
    async fn read_loop_rejects_oversized_content_before_allocating_body() {
        let input = format!("Content-Length: {}\r\n\r\n", MAX_CONTENT_LENGTH + 1);
        let (mut writer, reader) = tokio::io::duplex(1024);
        writer.write_all(input.as_bytes()).await.unwrap();
        drop(writer);

        let (tx, _rx) = mpsc::channel(1);
        let error = read_loop(reader, tx).await.unwrap_err();

        assert!(error.to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn read_loop_rejects_unbounded_header_line() {
        let input = vec![b'x'; MAX_HEADER_LINE_BYTES + 1];
        let reader = BufReader::new(input.as_slice());
        let (tx, _rx) = mpsc::channel(1);
        let error = read_loop(reader, tx).await.unwrap_err();

        assert!(error.to_string().contains("header line exceeds"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminate_closes_stdin_and_reaps_child() {
        let args = vec!["-c".to_string(), "read _".to_string()];
        let (mut transport, _incoming) = LspTransport::spawn("sh", &args, None).unwrap();

        transport.terminate().await.unwrap();

        assert!(transport.terminated);
        assert!(transport.writer.is_none());
        assert!(transport.child.try_wait().unwrap().is_some());
    }
}
