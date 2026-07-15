use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use adaptive_llama::{
    auto_gpu_layers, max_context_for_vram, query_vram_mb,
    resolve_llama_server_path,
};

use crate::types::{ServerConfig, ServerStatus};

// Global server process handle — no unsafe, no static mut.
static SERVER: Mutex<Option<Child>> = Mutex::new(None);

// The config the server was last started with. Lets the Start button relaunch
// the server in chat-via-server mode, where there is no GUI-loaded local
// instance to rebuild the config from. Kept across stop_server() — overwritten
// only on the next successful start.
static LAST_CONFIG: Mutex<Option<ServerConfig>> = Mutex::new(None);

/// Record the config a start used, for a later restart. Best-effort.
fn remember_config(config: &ServerConfig) {
    if let Ok(mut lc) = LAST_CONFIG.lock() {
        *lc = Some(config.clone());
    }
}

/// The config the server was last started with (GUI load OR magazine/chat-via-
/// server swap — all starts funnel through start_server). None only before the
/// very first start; survives stop() so Start can relaunch what last ran.
pub fn last_server_config() -> Option<ServerConfig> {
    LAST_CONFIG.lock().ok().and_then(|g| g.clone())
}

/// Start llama-server with auto-calculated flags.
///
/// Binds to 127.0.0.1:8081 (internal; the proxy sits on :8080).
pub fn start_server(config: &ServerConfig) -> Result<(), String> {
    let mut guard = SERVER.lock().map_err(|_| "server mutex poisoned".to_string())?;

    // If a server is already running, stop it first.
    if let Some(ref mut child) = *guard {
        let _ = child.kill();
        let _ = child.wait();
    }
    *guard = None;

    let bin = resolve_llama_server_path();

    let vram_mb = query_vram_mb();
    let ctx = config.context_length;
    let (gpu_layers, _) = auto_gpu_layers(&config.model_path, vram_mb, ctx);
    let capped_ctx = max_context_for_vram(&config.model_path, vram_mb)
        .unwrap_or(ctx)
        .min(ctx);

    // Apply app profile context cap if set (further clamp).
    let capped_ctx = if let Some(cap) = config.ctx_cap_override {
        capped_ctx.min(cap)
    } else {
        capped_ctx
    };

    let mut cmd = Command::new(&bin);
    cmd.arg("--host").arg("127.0.0.1")
       .arg("--port").arg("8081")
       .arg("-m").arg(&config.model_path)
       .arg("--ctx-size").arg(capped_ctx.to_string())
       .arg("--n-gpu-layers").arg(gpu_layers.to_string())
       .arg("--threads").arg(config.threads.to_string());

    if let Some(ref mp) = config.mmproj_path {
        cmd.arg("--mmproj").arg(mp);
    }
    if let Some(temp) = config.temperature_override {
        cmd.arg("--temp").arg(temp.to_string());
    }
    if let Some(n) = config.n_predict_override {
        cmd.arg("-n").arg(n.to_string());
    }

    cmd.stdout(Stdio::null())
       .stderr(Stdio::null());

    // Suppress console window on Windows.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let child = cmd.spawn().map_err(|e| format!("failed to spawn llama-server: {e}"))?;
    *guard = Some(child);
    remember_config(config);   // enable Start to relaunch this in server mode
    Ok(())
}

/// Stop the running llama-server (graceful: kill, then wait).
pub fn stop_server() -> Result<(), String> {
    let mut guard = SERVER.lock().map_err(|_| "server mutex poisoned".to_string())?;
    if let Some(ref mut child) = *guard {
        let _ = child.kill();
        let _ = child.wait();
    }
    *guard = None;
    Ok(())
}

/// Query the server health endpoint and return status.
///
/// This is a synchronous HTTP GET — it uses std::net::TcpStream to avoid
/// pulling in an async HTTP crate.  Returns `ServerStatus::Down` on any error.
pub fn health_check() -> ServerStatus {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    // Check whether the process is still alive first.
    {
        let mut guard = match SERVER.lock() {
            Ok(g) => g,
            Err(_) => return ServerStatus::Down,
        };
        match *guard {
            None => return ServerStatus::Stopped,
            Some(ref mut child) => {
                // try_wait: None = still running, Some = exited
                match child.try_wait() {
                    Ok(Some(_)) => {
                        *guard = None;
                        return ServerStatus::Crashed;
                    }
                    Ok(None) => {} // still running
                    Err(_) => return ServerStatus::Down,
                }
            }
        }
    }

    // Process alive — try a quick HTTP GET /health.
    let stream = match TcpStream::connect_timeout(
        &"127.0.0.1:8081".parse().unwrap(),
        Duration::from_millis(500),
    ) {
        Ok(s) => s,
        Err(_) => return ServerStatus::Starting,
    };

    let mut stream = stream;
    let req = b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
    if stream.write_all(req).is_err() {
        return ServerStatus::Starting;
    }

    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);

    if buf.contains("\"ok\"") {
        ServerStatus::Running
    } else if buf.contains("\"loading\"") {
        ServerStatus::Starting
    } else {
        ServerStatus::Starting
    }
}

/// Derive a model name slug from the GGUF file path (filename without extension).
pub fn model_name_from_path(model_path: &PathBuf) -> String {
    model_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("local")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(path: &str) -> ServerConfig {
        ServerConfig {
            model_path: PathBuf::from(path),
            context_length: 4096,
            threads: 9,
            mmproj_path: None,
            temperature_override: None,
            n_predict_override: None,
            ctx_cap_override: None,
        }
    }

    #[test]
    fn remembered_config_roundtrips_for_server_mode_restart() {
        // This is what lets the panel Start relaunch the server in chat-via-server
        // mode (no GUI instance): the last-run config is retained and returned.
        remember_config(&cfg(r"C:\m\gemma4-v2-Q8_0.gguf"));
        let got = last_server_config().expect("a config was remembered");
        assert_eq!(got.model_path, PathBuf::from(r"C:\m\gemma4-v2-Q8_0.gguf"));
        assert_eq!(got.context_length, 4096);
        // A later start overwrites it (a fresh swap wins).
        remember_config(&cfg(r"C:\m\Qwythos.gguf"));
        assert_eq!(
            last_server_config().unwrap().model_path,
            PathBuf::from(r"C:\m\Qwythos.gguf")
        );
    }
}
