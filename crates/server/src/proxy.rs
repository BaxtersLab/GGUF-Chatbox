/// Tool-calling proxy.
///
/// Listens on localhost:8080 (public-facing OpenAI-compatible endpoint).
/// Forwards requests to llama-server on localhost:8081.
/// If the model response contains `tool_calls`, dispatches them through
/// the tool_belt registry and re-queries until the response is plain text
/// or max_iterations is reached.
///
/// The proxy is intentionally simple: synchronous, single-request-at-a-time,
/// no async runtime required.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use serde_json::Value;

use crate::types::ToolDispatcher;

const MAX_TOOL_ITERATIONS: usize = 5;
const UPSTREAM_ADDR: &str = "127.0.0.1:8081";

// Path B (config-independent tool calls): extract an a6-tool fenced block from a
// model reply and return its JSON object. Lets a model that does NOT emit native
// OpenAI `tool_calls` (e.g. a llama-server started without `--jinja`) still call
// a tool by writing a fenced block, which the proxy loop then dispatches exactly
// like a native call.
//
// The block is a markdown fence tagged `a6-tool`; its body is the tool's
// arguments as a JSON object, with an optional `"tool"` field naming the tool
// (default `vscodium_workspace`). For example, a fence tagged a6-tool wrapping:
//   { "op": "create_folder", "path": "src/models" }
//
// Returns None when there is no block or its body is not a JSON object.
fn extract_a6_tool(content: &str) -> Option<serde_json::Value> {
    let marker = "```a6-tool";
    let start = content.find(marker)?;
    // Skip to the end of the fence-open line (past any trailing space + newline).
    let after = &content[start + marker.len()..];
    let body_start = after.find('\n')? + 1;
    let rest = &after[body_start..];
    let end = rest.find("```")?;
    let json_str = rest[..end].trim();
    let v: Value = serde_json::from_str(json_str).ok()?;
    if v.is_object() { Some(v) } else { None }
}

/// Start the proxy listener on localhost:8080.
/// Each connection is handled in its own thread.
///
/// `dispatcher` must be `Send + Sync + 'static` so it can be shared across threads.
pub fn start_proxy<D: ToolDispatcher + Send + Sync + 'static>(dispatcher: D) {
    if let Err(e) = start_proxy_on("127.0.0.1:8080", UPSTREAM_ADDR, dispatcher) {
        eprintln!("[proxy] bind failed: {e}");
    }
}

/// Start the proxy on an explicit listen address, forwarding to an explicit
/// upstream address. Returns `Err` if the listen address cannot be bound
/// (e.g. the port is already taken); on success it blocks serving connections.
///
/// `start_proxy` delegates here with the default 8080/8081 addresses — the
/// explicit form exists for tests and for future multi-slot listeners.
pub fn start_proxy_on<D: ToolDispatcher + Send + Sync + 'static>(
    addr: &str,
    upstream: &str,
    dispatcher: D,
) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr} failed: {e}"))?;

    use std::sync::Arc;
    let dispatcher = Arc::new(dispatcher);
    let upstream = Arc::new(upstream.to_string());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let d = Arc::clone(&dispatcher);
                let u = Arc::clone(&upstream);
                thread::spawn(move || handle_connection(stream, d.as_ref(), &u));
            }
            Err(_) => break,
        }
    }
    Ok(())
}

// ── Connection handler ────────────────────────────────────────────────────

fn handle_connection<D: ToolDispatcher>(mut client: TcpStream, dispatcher: &D, upstream: &str) {
    let (method, path, headers, body) = match read_request(&mut client) {
        Some(r) => r,
        None => return,
    };

    if method != "POST" || path != "/v1/chat/completions" {
        // Pass through everything else unchanged.
        let response = forward_raw(upstream, &method, &path, &headers, &body);
        let _ = client.write_all(response.as_bytes());
        return;
    }

    // Parse the body as a JSON chat completions request.
    let mut payload: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            let _ = client.write_all(bad_request().as_bytes());
            return;
        }
    };

    // Inject n_predict: -1 when neither n_predict nor max_tokens is set by the caller.
    // This prevents llama-server from using its default token cap (typically 512),
    // which causes the model to be cut off mid-response.
    // Safe for all upstream clients: skipped entirely if they explicitly set max_tokens.
    if payload.get("n_predict").is_none() && payload.get("max_tokens").is_none() {
        payload["n_predict"] = serde_json::json!(-1);
    }

    // Tool-calling loop — forward → check tool_calls → dispatch → repeat.
    let mut iterations = 0;
    loop {
        let upstream_body = payload.to_string();
        let response_body = match forward_json(upstream, &upstream_body) {
            Ok(r) => r,
            Err(e) => {
                let _ = client.write_all(error_response(&e).as_bytes());
                return;
            }
        };

        let response_json: Value = match serde_json::from_str(&response_body) {
            Ok(v) => v,
            Err(_) => {
                // Non-JSON upstream response — pass through as-is.
                let _ = client.write_all(http_200(&response_body).as_bytes());
                return;
            }
        };

        // Check for tool_calls in the first choice.
        let tool_calls = response_json
            .pointer("/choices/0/message/tool_calls")
            .and_then(|v| v.as_array())
            .cloned();

        match tool_calls {
            Some(calls) if !calls.is_empty() && iterations < MAX_TOOL_ITERATIONS => {
                iterations += 1;

                // Append the assistant message with tool_calls.
                let assistant_msg = response_json
                    .pointer("/choices/0/message")
                    .cloned()
                    .unwrap_or(Value::Null);
                if let Some(msgs) = payload["messages"].as_array_mut() {
                    msgs.push(assistant_msg);
                }

                // Dispatch each tool call and append tool-role messages.
                for call in &calls {
                    let id   = call["id"].as_str().unwrap_or("").to_string();
                    let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                    let args: Value = call["function"]["arguments"]
                        .as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(Value::Object(Default::default()));

                    let result = dispatcher.dispatch(&name, args)
                        .unwrap_or_else(|e| format!("Tool error: {e}"));

                    let tool_msg = serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": result,
                    });
                    if let Some(msgs) = payload["messages"].as_array_mut() {
                        msgs.push(tool_msg);
                    }
                }
                // Loop — re-query with updated messages.
            }
            _ => {
                // No native tool_calls. Path B: if the model emitted an
                // ```a6-tool``` block, dispatch it and re-query — the same
                // loop, for models/servers that don't produce OpenAI tool_calls.
                let fenced = response_json
                    .pointer("/choices/0/message/content")
                    .and_then(|v| v.as_str())
                    .and_then(extract_a6_tool);

                match fenced {
                    Some(mut call) if iterations < MAX_TOOL_ITERATIONS => {
                        iterations += 1;

                        // Append the assistant message that carried the block.
                        let assistant_msg = response_json
                            .pointer("/choices/0/message")
                            .cloned()
                            .unwrap_or(Value::Null);
                        if let Some(msgs) = payload["messages"].as_array_mut() {
                            msgs.push(assistant_msg);
                        }

                        // tool name (default vscodium_workspace); the rest is args.
                        let tool_name = call
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .unwrap_or("vscodium_workspace")
                            .to_string();
                        if let Some(obj) = call.as_object_mut() {
                            obj.remove("tool");
                        }

                        let result = dispatcher
                            .dispatch(&tool_name, call)
                            .unwrap_or_else(|e| format!("Tool error: {e}"));

                        // No tool_call_id exists in fenced mode, so a `tool`-role
                        // message (which needs one) would be malformed — convey the
                        // result as a user turn instead.
                        let result_msg = serde_json::json!({
                            "role": "user",
                            "content": format!("Tool result ({tool_name}): {result}"),
                        });
                        if let Some(msgs) = payload["messages"].as_array_mut() {
                            msgs.push(result_msg);
                        }
                        // Loop — re-query with the result in context.
                    }
                    _ => {
                        // No tool call of either kind (or max iterations) — return.
                        let _ = client.write_all(http_200(&response_body).as_bytes());
                        return;
                    }
                }
            }
        }
    }
}

// ── HTTP helpers ──────────────────────────────────────────────────────────

/// Read a full HTTP/1.x request from the stream.
/// Returns (method, path, headers, body).
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let mut parts = first_line.trim().splitn(3, ' ');
    let method = parts.next()?.to_string();
    let path   = parts.next()?.to_string();

    // Read headers until blank line.
    let mut headers = String::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line == "\r\n" || line == "\n" { break; }
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            content_length = lower
                .trim_start_matches("content-length:")
                .trim()
                .parse()
                .unwrap_or(0);
        }
        headers.push_str(&line);
    }

    // Read body.
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    let body = String::from_utf8_lossy(&body).into_owned();

    Some((method, path, headers, body))
}

/// Forward a JSON body to the upstream llama-server. Returns the response body.
fn forward_json(upstream: &str, body: &str) -> Result<String, String> {
    use std::time::Duration;
    let target = upstream
        .parse()
        .map_err(|e| format!("bad upstream address {upstream}: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(5))
        .map_err(|e| format!("upstream connect failed: {e}"))?;

    let req = format!(
        "POST /v1/chat/completions HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw).map_err(|e| e.to_string())?;

    // Strip HTTP headers, return only the body.
    if let Some(idx) = raw.find("\r\n\r\n") {
        Ok(raw[idx + 4..].to_string())
    } else {
        Ok(raw)
    }
}

/// Forward an arbitrary request to upstream unchanged and return the raw response.
fn forward_raw(upstream: &str, method: &str, path: &str, headers: &str, body: &str) -> String {
    use std::time::Duration;
    let target = match upstream.parse() {
        Ok(t) => t,
        Err(_) => return bad_gateway(),
    };
    let mut stream = match TcpStream::connect_timeout(&target, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(_) => return bad_gateway(),
    };
    let req = format!(
        "{} {} HTTP/1.0\r\nHost: 127.0.0.1\r\n{}Content-Length: {}\r\n\r\n{}",
        method, path, headers, body.len(), body
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return bad_gateway();
    }
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    raw
}

fn http_200(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body
    )
}

fn bad_request() -> String {
    let body = r#"{"error":"bad request"}"#;
    format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body
    )
}

fn bad_gateway() -> String {
    let body = r#"{"error":"upstream unavailable"}"#;
    format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body
    )
}

fn error_response(msg: &str) -> String {
    let body = format!(r#"{{"error":"{}"}}"#, msg.replace('"', "'"));
    format!(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body
    )
}

#[cfg(test)]
mod tests {
    use super::extract_a6_tool;

    #[test]
    fn extracts_a_flat_block() {
        let content = "Sure, creating it.\n```a6-tool\n{ \"op\": \"create_folder\", \"path\": \"src/models\" }\n```\nDone.";
        let v = extract_a6_tool(content).expect("should find the block");
        assert_eq!(v["op"], "create_folder");
        assert_eq!(v["path"], "src/models");
    }

    #[test]
    fn honors_an_optional_tool_field() {
        let content = "```a6-tool\n{\"tool\":\"vscodium_workspace\",\"op\":\"list_workspace_folders\"}\n```";
        let v = extract_a6_tool(content).unwrap();
        assert_eq!(v["tool"], "vscodium_workspace");
        assert_eq!(v["op"], "list_workspace_folders");
    }

    #[test]
    fn none_when_no_block() {
        assert!(extract_a6_tool("just a normal reply, no tools here").is_none());
    }

    #[test]
    fn none_when_block_body_is_not_json_object() {
        assert!(extract_a6_tool("```a6-tool\nnot json\n```").is_none());
        assert!(extract_a6_tool("```a6-tool\n[1,2,3]\n```").is_none());
    }

    #[test]
    fn none_when_fence_unclosed() {
        assert!(extract_a6_tool("```a6-tool\n{\"op\":\"x\"}").is_none());
    }
}
