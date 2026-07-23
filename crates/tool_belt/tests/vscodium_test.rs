//! VscodiumTool: the GGUF-Chatbox side of the Agent-6 tool bridge. These tests
//! drive it against a SIMULATED extension (a thread that watches the temp inbox
//! and writes a matching outbox result), so they're fast and need no real
//! VSCodium. The real extension is verified separately, end to end.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tool_belt::{ToolHandler, VscodiumTool};

/// Unique temp bridge dir for one test.
fn temp_bridge() -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gguf_vscodium_test_{}_{}", std::process::id(), n));
    std::fs::create_dir_all(dir.join("inbox")).unwrap();
    std::fs::create_dir_all(dir.join("outbox")).unwrap();
    std::fs::create_dir_all(dir.join("processed")).unwrap();
    dir
}

/// Spawn a stand-in for the VSCodium extension: watch the inbox for one command,
/// record it, and write the responder's result to `outbox/<id>.json`. Returns the
/// captured-commands handle; the thread exits after handling one command or ~5s.
fn spawn_sim(
    dir: &Path,
    responder: impl Fn(&Value) -> Value + Send + 'static,
) -> Arc<Mutex<Vec<Value>>> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_t = Arc::clone(&captured);
    let inbox = dir.join("inbox");
    let outbox = dir.join("outbox");
    let processed = dir.join("processed");
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let entries = std::fs::read_dir(&inbox).ok();
            if let Some(entries) = entries {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue; // skip .tmp
                    }
                    let text = match std::fs::read_to_string(&p) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let cmd: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let id = cmd["id"].as_str().unwrap_or("no-id").to_string();
                    captured_t.lock().unwrap().push(cmd.clone());
                    let result = responder(&cmd);
                    // Publish atomically like the real extension.
                    let out = outbox.join(format!("{id}.json"));
                    let tmp = outbox.join(format!("{id}.json.tmp"));
                    std::fs::write(&tmp, result.to_string()).unwrap();
                    std::fs::rename(&tmp, &out).unwrap();
                    let _ = std::fs::rename(&p, processed.join(p.file_name().unwrap()));
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    captured
}

#[test]
fn create_folder_roundtrips_and_returns_result_payload() {
    let dir = temp_bridge();
    let captured = spawn_sim(&dir, |cmd| {
        let id = cmd["id"].as_str().unwrap();
        let path = cmd["args"]["path"].as_str().unwrap_or("");
        json!({ "id": id, "op": "create_folder", "ok": true,
                "result": { "path": format!("/ws/{path}") } })
    });

    let tool = VscodiumTool::with_config(dir.clone(), Duration::from_secs(4));
    let out = tool.execute(json!({ "op": "create_folder", "path": "src/models" })).unwrap();

    // The model receives the `result` payload as JSON.
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["path"], "/ws/src/models");

    // The command the extension saw: correct op, and `op` stripped from args.
    let cmds = captured.lock().unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0]["op"], "create_folder");
    assert_eq!(cmds[0]["args"]["path"], "src/models");
    assert!(cmds[0]["args"].get("op").is_none(), "op must not leak into args");
    assert!(cmds[0]["id"].is_string(), "command must carry an id");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extension_error_becomes_a_tool_error() {
    let dir = temp_bridge();
    spawn_sim(&dir, |cmd| {
        let id = cmd["id"].as_str().unwrap();
        json!({ "id": id, "op": "create_folder", "ok": false,
                "error": "path escapes the workspace: /etc" })
    });

    let tool = VscodiumTool::with_config(dir.clone(), Duration::from_secs(4));
    let err = tool
        .execute(json!({ "op": "create_folder", "path": "../../etc" }))
        .unwrap_err();
    assert!(err.to_string().contains("escapes the workspace"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn times_out_when_no_extension_answers() {
    let dir = temp_bridge();
    // No sim spawned — nothing writes the outbox.
    let tool = VscodiumTool::with_config(dir.clone(), Duration::from_millis(300));
    let err = tool
        .execute(json!({ "op": "list_workspace_folders" }))
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_op_is_rejected_before_dispatch() {
    let dir = temp_bridge();
    let tool = VscodiumTool::with_config(dir.clone(), Duration::from_millis(300));
    let err = tool.execute(json!({ "op": "delete_everything" })).unwrap_err();
    assert!(err.to_string().contains("unknown vscodium op"), "got: {err}");
    // Nothing should have been written to the inbox.
    let count = std::fs::read_dir(dir.join("inbox")).unwrap().count();
    assert_eq!(count, 0, "a bad op must not write a command");

    let _ = std::fs::remove_dir_all(&dir);
}

/// End-to-end against a LIVE extension. Ignored by default (needs a real
/// VSCodium running the A6 tools extension). The E2E script launches VSCodium,
/// sets A6_TOOLS_DIR to the live bridge, then runs this with `-- --ignored`.
#[test]
#[ignore]
fn real_extension_create_folder() {
    let bridge = std::env::var("A6_TOOLS_DIR")
        .expect("set A6_TOOLS_DIR to the live bridge dir");
    let tool = VscodiumTool::with_config(PathBuf::from(bridge), Duration::from_secs(20));
    let out = tool
        .execute(json!({ "op": "create_folder", "path": "rust_bridge_proof" }))
        .expect("live extension should create the folder");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["path"].as_str().unwrap_or("").ends_with("rust_bridge_proof"),
        "unexpected result: {out}"
    );
    eprintln!("REAL create_folder result: {out}");
}

#[test]
fn missing_op_argument_errors() {
    let dir = temp_bridge();
    let tool = VscodiumTool::with_config(dir.clone(), Duration::from_millis(300));
    let err = tool.execute(json!({ "path": "x" })).unwrap_err();
    assert!(err.to_string().contains("'op' argument is required"), "got: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}
