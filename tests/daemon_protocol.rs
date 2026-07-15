use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

    assert!(events
        .iter()
        .any(|event| event["type"] == "initialized" && event["id"] == 7));
    assert!(events
        .iter()
        .any(|event| event["type"] == "shutdown" && event["id"] == 8));
    let _ = std::fs::remove_dir_all(workspace);
}
