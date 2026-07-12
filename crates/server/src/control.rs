//! Additive swap-control endpoint for the CD-changer.
//!
//! Listens on a NEW port (127.0.0.1:8086) — the proxy (:8080) and llama-server
//! (:8081) are left completely untouched (per the "only ADD, never change
//! existing" rule for this shared public repo). It lets an external orchestrator
//! (SOC Ultralight) trigger a model swap remotely, reusing the existing
//! `server_manager::start_server` primitive.
//!
//! Routes:
//!   POST /swap   {"model_path": "...", "mmproj_path": "..."|null,
//!                 "context_length": <opt u32>, "threads": <opt u32>}
//!     -> kills the running llama-server and respawns with this model.
//!        Idempotent: if the requested model is already the loaded one, it does
//!        NOT restart (returns {"ok":true,"already":true}) — this is what makes
//!        SOC's re-dispatch / double-clicks safe (no needless context loss).
//!   GET  /swap/status
//!     -> {"status":"Running|Starting|...","loaded_model":"<path>"}
//!
//! Synchronous, one-thread-per-connection, std-only (matches proxy.rs).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::server_manager::{health_check, start_server};
use crate::types::ServerConfig;

const CONTROL_ADDR: &str = "127.0.0.1:8086";
const UPSTREAM_ADDR: &str = "127.0.0.1:8081";

/// Body of a POST /swap request. `context_length`/`threads` are optional so the
/// caller only has to name the disk (model + mmproj); sane defaults fill the rest.
#[derive(Debug, Deserialize)]
struct SwapRequest {
    model_path: PathBuf,
    #[serde(default)]
    mmproj_path: Option<PathBuf>,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    threads: Option<u32>,
}

const DEFAULT_CTX: u32 = 8192;
const DEFAULT_THREADS: u32 = 8;

/// Build a `ServerConfig` from a swap request, applying defaults. Pure — testable.
fn build_config(req: SwapRequest) -> ServerConfig {
    ServerConfig {
        model_path: req.model_path,
        context_length: req.context_length.unwrap_or(DEFAULT_CTX),
        threads: req.threads.unwrap_or(DEFAULT_THREADS),
        mmproj_path: req.mmproj_path,
        temperature_override: None,
        n_predict_override: None,
        ctx_cap_override: None,
    }
}

/// Case-insensitive, slash-normalized path comparison (Windows-friendly). Pure.
fn paths_equal(a: &PathBuf, b: &PathBuf) -> bool {
    let norm = |p: &PathBuf| p.to_string_lossy().replace('\\', "/").to_lowercase();
    norm(a) == norm(b)
}

/// Start the control listener on :8086. Blocks serving connections; run in a thread.
pub fn start_control_server() {
    match TcpListener::bind(CONTROL_ADDR) {
        Ok(listener) => {
            for stream in listener.incoming() {
                if let Ok(s) = stream {
                    thread::spawn(move || handle_connection(s));
                }
            }
        }
        Err(e) => eprintln!("[control] bind {CONTROL_ADDR} failed: {e}"),
    }
}

fn handle_connection(mut client: TcpStream) {
    let (method, path, body) = match read_request(&mut client) {
        Some(r) => r,
        None => return,
    };
    let response = route(&method, &path, &body);
    let _ = client.write_all(response.as_bytes());
}

fn route(method: &str, path: &str, body: &str) -> String {
    match (method, path) {
        ("POST", "/swap") => do_swap(body),
        ("GET", "/swap/status") => swap_status(),
        _ => json_response(404, r#"{"ok":false,"error":"not found"}"#),
    }
}

fn do_swap(body: &str) -> String {
    let req: SwapRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            return json_response(
                400,
                &format!(r#"{{"ok":false,"error":"bad request: {}"}}"#, esc(&e.to_string())),
            )
        }
    };

    // Idempotency: skip the restart (and the context loss it causes) if the
    // requested disk is already loaded.
    if let Some(loaded) = loaded_model_path() {
        if paths_equal(&loaded, &req.model_path) {
            return json_response(200, r#"{"ok":true,"already":true}"#);
        }
    }

    let cfg = build_config(req);
    match start_server(&cfg) {
        Ok(()) => json_response(200, r#"{"ok":true,"loading":true}"#),
        Err(e) => json_response(
            500,
            &format!(r#"{{"ok":false,"error":"{}"}}"#, esc(&e)),
        ),
    }
}

fn swap_status() -> String {
    let status = format!("{:?}", health_check());
    let loaded = loaded_model_path()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    json_response(
        200,
        &format!(r#"{{"status":"{}","loaded_model":"{}"}}"#, status, esc(&loaded)),
    )
}

/// Ground-truth loaded model: ask llama-server (:8081) /v1/models. Returns None
/// if the server is down or the shape is unrecognized.
fn loaded_model_path() -> Option<PathBuf> {
    let raw = raw_get(UPSTREAM_ADDR, "/v1/models")?;
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or(&raw);
    let v: Value = serde_json::from_str(body).ok()?;
    let path = v
        .pointer("/models/0/model")
        .or_else(|| v.pointer("/models/0/name"))
        .or_else(|| v.pointer("/data/0/id"))
        .and_then(|x| x.as_str())?;
    Some(PathBuf::from(path))
}

// ── HTTP helpers (std-only, mirrors proxy.rs) ───────────────────────────────

fn read_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let mut parts = first_line.trim().splitn(3, ' ');
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            content_length = lower
                .trim_start_matches("content-length:")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }

    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).ok()?;
    Some((method, path, String::from_utf8_lossy(&buf).into_owned()))
}

/// Raw HTTP GET to an upstream, returning the raw response (headers + body).
fn raw_get(upstream: &str, path: &str) -> Option<String> {
    let target = upstream.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(3)).ok()?;
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    Some(raw)
}

fn json_response(code: u16, body: &str) -> String {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// Escape a string for embedding inside a JSON string literal.
fn esc(s: &str) -> String {
    s.replace('\\', "/").replace('"', "'").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_applies_defaults() {
        let req = SwapRequest {
            model_path: PathBuf::from("/m/model.gguf"),
            mmproj_path: None,
            context_length: None,
            threads: None,
        };
        let cfg = build_config(req);
        assert_eq!(cfg.context_length, DEFAULT_CTX);
        assert_eq!(cfg.threads, DEFAULT_THREADS);
        assert!(cfg.mmproj_path.is_none());
    }

    #[test]
    fn build_config_keeps_overrides_and_mmproj() {
        let req = SwapRequest {
            model_path: PathBuf::from("/m/vision.gguf"),
            mmproj_path: Some(PathBuf::from("/m/mmproj.gguf")),
            context_length: Some(16384),
            threads: Some(12),
        };
        let cfg = build_config(req);
        assert_eq!(cfg.context_length, 16384);
        assert_eq!(cfg.threads, 12);
        assert_eq!(cfg.mmproj_path, Some(PathBuf::from("/m/mmproj.gguf")));
    }

    #[test]
    fn parse_swap_request_minimal() {
        let req: SwapRequest =
            serde_json::from_str(r#"{"model_path":"/m/a.gguf"}"#).unwrap();
        assert_eq!(req.model_path, PathBuf::from("/m/a.gguf"));
        assert!(req.mmproj_path.is_none());
    }

    #[test]
    fn paths_equal_is_case_and_slash_insensitive() {
        assert!(paths_equal(
            &PathBuf::from(r"C:\Models\Qwythos.gguf"),
            &PathBuf::from("c:/models/qwythos.gguf"),
        ));
        assert!(!paths_equal(
            &PathBuf::from("/m/a.gguf"),
            &PathBuf::from("/m/b.gguf"),
        ));
    }

    #[test]
    fn route_unknown_is_404() {
        assert!(route("GET", "/nope", "").contains("404"));
    }
}
