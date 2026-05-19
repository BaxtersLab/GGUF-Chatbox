use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use serde_json::{json, Value};
use tool_belt::ToolRegistry;

static MCP_RUNNING: AtomicBool = AtomicBool::new(false);

/// Start the MCP server on port 8082 with the default tool registry.
/// No-op if already running.
pub fn start_mcp_server() {
    if MCP_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let registry = Arc::new(Mutex::new(tool_belt::default_registry()));
    thread::spawn(move || run(registry));
}

/// Stop the MCP server. The server thread exits on the next request cycle.
pub fn stop_mcp_server() {
    MCP_RUNNING.store(false, Ordering::SeqCst);
}

fn run(registry: Arc<Mutex<ToolRegistry>>) {
    let server = match tiny_http::Server::http("127.0.0.1:8084") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[mcp] bind failed: {e}");
            MCP_RUNNING.store(false, Ordering::SeqCst);
            return;
        }
    };
    eprintln!("[mcp] listening on 127.0.0.1:8084");

    for mut req in server.incoming_requests() {
        if !MCP_RUNNING.load(Ordering::Relaxed) {
            break;
        }
        let mut body = String::new();
        req.as_reader().read_to_string(&mut body).ok();
        let response_json = dispatch(&body, &registry);
        let response = tiny_http::Response::from_string(response_json)
            .with_header(
                "Content-Type: application/json"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            );
        let _ = req.respond(response);
    }
    MCP_RUNNING.store(false, Ordering::SeqCst);
    eprintln!("[mcp] stopped");
}

fn dispatch(body: &str, registry: &Arc<Mutex<ToolRegistry>>) -> String {
    let req: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return rpc_error(-32700, "parse error", Value::Null),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req["method"].as_str().unwrap_or("");

    match method {
        "initialize" => rpc_ok(
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "gguf-chatbox-mcp", "version": "0.1.0" }
            }),
            id,
        ),
        "initialized" => rpc_ok(json!({}), id),
        "tools/list" => {
            let reg = registry.lock().unwrap();
            let tools: Vec<Value> = reg
                .list_tools()
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.parameters_schema
                    })
                })
                .collect();
            rpc_ok(json!({ "tools": tools }), id)
        }
        "tools/call" => {
            let params = &req["params"];
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let reg = registry.lock().unwrap();
            match reg.dispatch(name, args) {
                Ok(text) => rpc_ok(
                    json!({ "content": [{ "type": "text", "text": text }] }),
                    id,
                ),
                Err(e) => rpc_error(-32000, &e.to_string(), id),
            }
        }
        _ => rpc_error(-32601, "method not found", id),
    }
}

fn rpc_ok(result: Value, id: Value) -> String {
    json!({ "jsonrpc": "2.0", "result": result, "id": id }).to_string()
}

fn rpc_error(code: i32, message: &str, id: Value) -> String {
    json!({ "jsonrpc": "2.0", "error": { "code": code, "message": message }, "id": id })
        .to_string()
}
