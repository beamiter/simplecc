use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn temporary_workspace(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "simplecc-daemon-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run_daemon(requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_simplecc-daemon"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    {
        let stdin = child.stdin.as_mut().unwrap();
        for request in requests {
            writeln!(stdin, "{}", serde_json::to_string(request).unwrap()).unwrap();
        }
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn initialize_and_shutdown_are_processed_in_wire_order() {
    let workspace = temporary_workspace("lifecycle");
    let events = run_daemon(&[
        json!({
            "type": "initialize",
            "id": 1,
            "root": workspace.to_string_lossy(),
        }),
        json!({ "type": "shutdown", "id": 2 }),
    ]);
    let lifecycle: Vec<_> = events
        .iter()
        .filter_map(|event| {
            let kind = event.get("type")?.as_str()?;
            matches!(kind, "initialized" | "shutdown")
                .then(|| (kind.to_string(), event["id"].as_u64().unwrap()))
        })
        .collect();

    assert_eq!(
        lifecycle,
        [("initialized".to_string(), 1), ("shutdown".to_string(), 2)]
    );
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn malformed_input_does_not_poison_the_next_request() {
    let workspace = temporary_workspace("malformed");
    let mut child = Command::new(env!("CARGO_BIN_EXE_simplecc-daemon"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "not-json").unwrap();
        writeln!(
            stdin,
            "{}",
            json!({
                "type": "initialize",
                "id": 7,
                "root": workspace.to_string_lossy(),
            })
        )
        .unwrap();
        writeln!(stdin, "{}", json!({ "type": "shutdown", "id": 8 })).unwrap();
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    // Surviving the bad line is half of it; saying so is the other half.  A
    // line that failed to deserialise used to produce nothing on the channel
    // at all, so the only record of it was one stderr line the frontend never
    // reads, and the caller waited out its 30 s request timeout instead.
    assert!(
        events.iter().any(|event| {
            event["type"] == "error"
                && event["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("invalid request:"))
        }),
        "a malformed line must be answered on the wire: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "initialized" && event["id"] == 7)
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "shutdown" && event["id"] == 8)
    );
    let _ = std::fs::remove_dir_all(workspace);
}

/// Protocol drift — a field renamed on one side of a partial upgrade, or a
/// `type` tag the daemon has never heard of — is well-formed JSON that does
/// not deserialise.  The reply has to name what failed, because the frontend's
/// only other outcome is a request timeout whose message says nothing about
/// the cause.
#[test]
fn a_request_the_daemon_cannot_decode_is_answered_with_the_reason() {
    let events = run_daemon(&[
        json!({ "type": "textDocument/definitelyNotAThing", "id": 12 }),
        json!({ "type": "shutdown", "id": 13 }),
    ]);
    let reason = events
        .iter()
        .find(|event| event["type"] == "error")
        .and_then(|event| event["message"].as_str())
        .unwrap_or_else(|| panic!("no error event for an undecodable request: {events:?}"));
    assert!(
        reason.starts_with("invalid request:"),
        "unexpected error message: {reason}"
    );
    assert!(
        reason.contains("textDocument/definitelyNotAThing"),
        "the error must name the tag that failed: {reason}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "shutdown" && event["id"] == 13)
    );
}

#[test]
fn accepted_feature_request_is_drained_after_eof() {
    // This request is dispatched on the feature-task set rather than handled
    // inline. Closing stdin immediately after writing it used to abort that
    // task during daemon shutdown and produce no reply at all.
    let events = run_daemon(&[json!({
        "type": "server/listInstallable",
        "id": 91,
    })]);
    assert!(events.iter().any(|event| {
        event["type"] == "installableServers" && event["id"] == 91 && event["servers"].is_array()
    }));
}

#[test]
fn unread_stdout_cannot_hold_eof_shutdown_forever() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_simplecc-daemon"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for id in 1..=256 {
            writeln!(
                stdin,
                "{}",
                json!({"type": "server/listInstallable", "id": id})
            )
            .unwrap();
        }
    }
    drop(child.stdin.take());

    // Keep stdout piped and unread.  The writer's dedicated-thread deadline
    // must let the process terminate even after the pipe fills.
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("daemon did not bound EOF shutdown under stdout backpressure");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(status.success(), "daemon exited unsuccessfully: {status}");
}

#[test]
fn ordered_did_open_cannot_block_eof_behind_stdout_backpressure() {
    let workspace = temporary_workspace("did-open-backpressure");
    let requests_path = workspace.join("requests.jsonl");
    {
        let mut requests = std::fs::File::create(&requests_path).unwrap();
        writeln!(
            requests,
            "{}",
            json!({
                "type": "initialize",
                "id": 1,
                "root": "/workspace",
                "remote": {
                    "kind": "ssh",
                    "target": "unused",
                    "root": "/workspace",
                    "runtime": "/definitely/not-a-simplecc-runtime"
                },
                "remoteConfig": serde_json::json!({
                    "languageServers": {
                        "stall-test": {
                            "command": "never-run",
                            "filetypes": ["stall-test"]
                        }
                    }
                }).to_string()
            })
        )
        .unwrap();
        // Fill the bounded stdout path before the ordered didOpen. The daemon
        // must not depend on PATH or a real language server to reach the
        // Registry status producer exercised below.
        for id in 2..=8_001 {
            writeln!(
                requests,
                "{}",
                json!({"type": "server/listInstallable", "id": id})
            )
            .unwrap();
        }
        writeln!(
            requests,
            "{}",
            json!({
                "type": "textDocument/didOpen",
                "id": 9_000,
                "uri": "file:///workspace/stall.test",
                "languageId": "stall-test",
                "version": 1,
                "text": "test"
            })
        )
        .unwrap();
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_simplecc-daemon"))
        .stdin(Stdio::from(std::fs::File::open(&requests_path).unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&workspace);
            panic!("ordered didOpen remained blocked behind stdout backpressure");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let _ = std::fs::remove_dir_all(&workspace);
    assert!(status.success(), "daemon exited unsuccessfully: {status}");
}

#[test]
fn stdout_stall_interrupts_an_open_idle_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_simplecc-daemon"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut sent = 0;
    for id in 1..=8_000 {
        match writeln!(
            stdin,
            "{}",
            json!({"type": "server/listInstallable", "id": id})
        ) {
            Ok(()) => sent += 1,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(error) => panic!("could not write request flood: {error}"),
        }
    }
    assert!(
        sent > 4_096,
        "fixture never exceeded the stdout channel capacity"
    );

    // Intentionally keep stdin open and send nothing else. The EventTx notify,
    // not EOF or another request, must wake the blocked input loop.
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("stdout stall did not interrupt an idle stdin read");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    drop(stdin);
    assert!(status.success(), "daemon exited unsuccessfully: {status}");
}
