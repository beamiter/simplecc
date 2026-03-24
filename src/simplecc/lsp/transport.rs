use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;

/// LSP transport: speaks Content-Length framed JSON-RPC over stdio of a child process.
pub struct LspTransport {
    writer: ChildStdin,
    _child: Child,
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
            .stderr(std::process::Stdio::piped());
        if let Some(dir) = root_dir {
            command.current_dir(dir);
        }

        let mut child = command.spawn().with_context(|| format!("failed to spawn {cmd}"))?;

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
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => eprintln!("[lsp-stderr] {}", line.trim_end()),
                    }
                }
            });
        }

        Ok((Self { writer, _child: child }, rx))
    }

    /// Send a JSON-RPC message with Content-Length header.
    pub async fn send(&mut self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(body.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

/// Read Content-Length framed messages from stdout.
async fn read_loop(stdout: ChildStdout, tx: mpsc::Sender<Value>) -> Result<()> {
    let mut reader = BufReader::new(stdout);
    let mut header_buf = String::new();

    loop {
        // Read headers
        let mut content_length: usize = 0;
        loop {
            header_buf.clear();
            let n = reader.read_line(&mut header_buf).await?;
            if n == 0 {
                return Ok(()); // EOF
            }
            let line = header_buf.trim();
            if line.is_empty() {
                break; // End of headers
            }
            if let Some(val) = line.strip_prefix("Content-Length:") {
                content_length = val.trim().parse().context("bad Content-Length")?;
            }
        }

        if content_length == 0 {
            bail!("missing Content-Length");
        }

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
