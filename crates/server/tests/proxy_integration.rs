//! Proxy behaviour tests, fully hermetic: every listener binds an ephemeral
//! localhost port, so these never touch the real 8080/8081 slots even while
//! the app is running.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use server::{start_proxy_on, NoopDispatcher, ToolDispatcher};

/// Reserve a free localhost port by binding :0 and immediately releasing it.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Spawn the proxy on a fresh port and wait until it accepts connections.
fn spawn_proxy<D: ToolDispatcher + Send + Sync + 'static>(upstream: String, dispatcher: D) -> String {
    let addr = format!("127.0.0.1:{}", free_port());
    let listen_addr = addr.clone();
    thread::spawn(move || {
        let _ = start_proxy_on(&listen_addr, &upstream, dispatcher);
    });
    for _ in 0..100 {
        if TcpStream::connect(&addr).is_ok() {
            return addr;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("proxy did not start on {addr}");
}

fn http_post(addr: &str, path: &str, body: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    let req = format!(
        "POST {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// Drain one HTTP request (headers + Content-Length body) from a stream.
fn read_http_request(s: &mut TcpStream) {
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.is_empty() {
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
}

/// Fake llama-server: serves the given JSON bodies in order, one connection
/// each, closing the socket after every response (HTTP/1.0 semantics).
fn fake_upstream(bodies: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for body in bodies {
            let (mut s, _) = match listener.accept() {
                Ok(c) => c,
                Err(_) => return,
            };
            read_http_request(&mut s);
            let resp = format!(
                "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    addr
}

struct RecordingDispatcher {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl ToolDispatcher for RecordingDispatcher {
    fn dispatch(&self, name: &str, args: Value) -> Result<String, String> {
        self.calls.lock().unwrap().push((name.to_string(), args));
        Ok("42".to_string())
    }
}

/// The REAL production dispatcher: the same tool_belt registry main.rs ships to
/// the proxy (system_info + file_ops + vscodium_workspace).
struct RealRegistryDispatcher(tool_belt::ToolRegistry);

impl ToolDispatcher for RealRegistryDispatcher {
    fn dispatch(&self, name: &str, args: Value) -> Result<String, String> {
        self.0.dispatch(name, args).map_err(|e| e.to_string())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[test]
fn bind_collision_returns_err() {
    let addr = format!("127.0.0.1:{}", free_port());
    let _holder = TcpListener::bind(&addr).unwrap();
    let result = start_proxy_on(&addr, "127.0.0.1:1", NoopDispatcher);
    assert!(result.is_err(), "second bind on {addr} must fail, not panic or hang");
}

#[test]
fn invalid_json_returns_400_without_upstream() {
    // Upstream is a dead port — the 400 must be produced before any upstream contact.
    let dead = format!("127.0.0.1:{}", free_port());
    let addr = spawn_proxy(dead, NoopDispatcher);
    let resp = http_post(&addr, "/v1/chat/completions", "{not json");
    assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
}

#[test]
fn dead_upstream_passthrough_yields_502() {
    let dead = format!("127.0.0.1:{}", free_port());
    let addr = spawn_proxy(dead, NoopDispatcher);
    // Non-chat paths are forwarded raw; with no upstream the proxy answers 502.
    let mut s = TcpStream::connect(&addr).unwrap();
    s.write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.contains("502"), "got: {out}");
}

#[test]
fn plain_response_passes_through_unchanged() {
    let final_response =
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":"hello"}}]})
            .to_string();
    let upstream = fake_upstream(vec![final_response]);
    let addr = spawn_proxy(upstream, NoopDispatcher);

    let resp = http_post(
        &addr,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"hi"}]}"#,
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    assert!(resp.contains("hello"), "got: {resp}");
}

#[test]
fn fenced_a6_tool_block_dispatches_then_returns_final_response() {
    // Path B: the model doesn't emit native tool_calls — it writes an ```a6-tool```
    // block in the message content. The proxy must extract it, dispatch, and
    // re-query, exactly like the native path.
    let block_turn = serde_json::json!({
        "choices": [{"message": {"role": "assistant",
            "content": "On it.\n```a6-tool\n{\"op\": \"create_folder\", \"path\": \"src/models\"}\n```"}}]
    })
    .to_string();
    let final_turn =
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":"folder created"}}]})
            .to_string();

    let upstream = fake_upstream(vec![block_turn, final_turn]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let addr = spawn_proxy(upstream, RecordingDispatcher { calls: Arc::clone(&calls) });

    let resp = http_post(
        &addr,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"make a models folder"}]}"#,
    );

    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    assert!(resp.contains("folder created"), "got: {resp}");

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "exactly one fenced dispatch expected");
    assert_eq!(calls[0].0, "vscodium_workspace", "default tool name");
    assert_eq!(calls[0].1["op"], "create_folder");
    assert_eq!(calls[0].1["path"], "src/models");
    assert!(calls[0].1.get("tool").is_none(), "the 'tool' selector must not leak into args");
}

#[test]
fn tool_call_loop_dispatches_then_returns_final_response() {
    // First upstream turn asks for a tool call; second returns plain text.
    let tool_turn = serde_json::json!({
        "choices": [{"message": {"role": "assistant", "content": null,
            "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "system_info", "arguments": "{\"detail\":\"basic\"}"}}]}}]
    })
    .to_string();
    let final_turn =
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":"the answer is 42"}}]})
            .to_string();

    let upstream = fake_upstream(vec![tool_turn, final_turn]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let addr = spawn_proxy(upstream, RecordingDispatcher { calls: Arc::clone(&calls) });

    let resp = http_post(
        &addr,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"hi"}]}"#,
    );

    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    assert!(resp.contains("the answer is 42"), "got: {resp}");

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "exactly one tool dispatch expected");
    assert_eq!(calls[0].0, "system_info");
    assert_eq!(calls[0].1["detail"], "basic");
}

// ── Full-chain live tests (ignored: need live services) ───────────────────────
// These wire the REAL production path end to end:
//   proxy (real code) → real tool_belt registry → VscodiumTool → the real
//   VSCodium extension → the workspace.
// Run via the orchestration script, which starts A6's VSCodium and sets:
//   A6_TOOLS_DIR  the live tool bridge dir the extension is watching
//   A6_UPSTREAM   a live llama-server "host:port" (real-model variant only)

/// Full wiring WITHOUT a model: a canned upstream turn carries the a6-tool block,
/// so this deterministically proves proxy → registry → VscodiumTool → extension.
#[test]
#[ignore]
fn full_chain_fenced_block_reaches_the_real_extension() {
    let folder = std::env::var("A6_PROOF_FOLDER").unwrap_or_else(|_| "chain_proof".to_string());
    let block_turn = serde_json::json!({
        "choices": [{"message": {"role": "assistant",
            "content": format!("Working on it.\n```a6-tool\n{{\"op\": \"create_folder\", \"path\": \"{folder}\"}}\n```")}}]
    })
    .to_string();
    let final_turn =
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":"done"}}]}).to_string();

    let upstream = fake_upstream(vec![block_turn, final_turn]);
    let addr = spawn_proxy(upstream, RealRegistryDispatcher(tool_belt::default_registry()));

    let resp = http_post(
        &addr,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"make it"}],"max_tokens":64}"#,
    );
    eprintln!("PROXY RESPONSE: {resp}");
    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    assert!(resp.contains("done"), "loop should re-query and return the final turn: {resp}");
}

/// Full chain WITH a real model: the model itself must emit the a6-tool block.
#[test]
#[ignore]
fn full_chain_real_model_emits_block_and_acts() {
    let upstream = std::env::var("A6_UPSTREAM").unwrap_or_else(|_| "127.0.0.1:8081".to_string());
    let folder = std::env::var("A6_PROOF_FOLDER").unwrap_or_else(|_| "model_proof".to_string());
    let addr = spawn_proxy(upstream, RealRegistryDispatcher(tool_belt::default_registry()));

    // Mirrors the real Agent-6 tool rail from soc_ultralight's _local_agent_header.
    let prompt = format!(
        "[SOC LOOP — you are Agent 6.]\n\
         WORKSPACE TOOL (Agent 6 only): to act on the workspace, output a fenced block \
         tagged a6-tool holding ONE flat JSON object. The system runs it and returns the \
         result. Example:\n\
         ```a6-tool\n{{\"op\": \"create_folder\", \"path\": \"src/models\"}}\n```\n\
         ops: create_folder, create_file (+\"content\"), read_file, list_dir.\n\n\
         TASK: create a folder named {folder} in the workspace. \
         Emit the a6-tool block now, nothing else."
    );
    let body = serde_json::json!({
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.1,
        "max_tokens": 300
    })
    .to_string();

    let resp = http_post(&addr, "/v1/chat/completions", &body);
    eprintln!("=== FULL PROXY RESPONSE ===\n{resp}\n=== END ===");
    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
}
