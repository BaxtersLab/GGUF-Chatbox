use std::process::Child;
use std::sync::Mutex;
use crate::errors::{LoaderResult, ModelError};
use crate::types::SamplingConfig;

/// Configuration for spawning a llama-server instance.
pub struct ServerConfig {
    pub model_path: std::path::PathBuf,
    /// Multimodal projector file (.gguf). Required for vision models.
    pub mmproj_path: Option<std::path::PathBuf>,
    pub context_length: u32,
    pub gpu_layers: u32,
    pub threads: u32,
    pub sampling: SamplingConfig,
    pub host: String,
    pub port: u16,
}

/// Health status of the llama-server process.
pub enum ServerStatus {
    Stopped,
    Starting,
    Running,
    Error(String),
}

/// Global handle to the managed llama-server subprocess.
/// Uses Mutex so it can be safely shared across threads — no unsafe, no static mut.
static SERVER_PROCESS: Mutex<Option<Child>> = Mutex::new(None);

/// Start the llama-server subprocess with the given configuration.
/// If a server is already running, this is a no-op.
pub fn start_server(config: &ServerConfig) -> LoaderResult<()> {
    let mut guard = SERVER_PROCESS
        .lock()
        .map_err(|_| ModelError::IoError("server lock poisoned".to_string()))?;

    if guard.is_some() {
        // Already running.
        return Ok(());
    }

    let server_path = crate::llama_detect::resolve_llama_server_path();
    let mut cmd = std::process::Command::new(&server_path);

    cmd.arg("--model")
        .arg(&config.model_path)
        .arg("--host")
        .arg(&config.host)
        .arg("--port")
        .arg(config.port.to_string())
        .arg("--ctx-size")
        .arg(config.context_length.to_string())
        .arg("--n-gpu-layers")
        .arg(config.gpu_layers.to_string())
        .arg("--threads")
        .arg(config.threads.to_string());

    if let Some(ref mp) = config.mmproj_path {
        cmd.arg("--mmproj").arg(mp);
    }

    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().map_err(|e| {
        ModelError::IoError(format!("failed to start llama-server: {}", e))
    })?;

    *guard = Some(child);
    Ok(())
}

/// Stop the llama-server subprocess if it is running.
pub fn stop_server() -> LoaderResult<()> {
    let mut guard = SERVER_PROCESS
        .lock()
        .map_err(|_| ModelError::IoError("server lock poisoned".to_string()))?;

    if let Some(ref mut child) = *guard {
        let _ = child.kill();
        let _ = child.wait();
    }
    *guard = None;
    Ok(())
}

/// Check whether the llama-server process is alive.
///
/// Returns `Stopped` if no process was started, `Running` if the process
/// handle is present and has not exited, or `Error` if the lock is poisoned.
pub fn health_check() -> ServerStatus {
    let mut guard = match SERVER_PROCESS.lock() {
        Ok(g) => g,
        Err(_) => return ServerStatus::Error("server lock poisoned".to_string()),
    };

    match *guard {
        None => ServerStatus::Stopped,
        Some(ref mut child) => {
            // try_wait returns Ok(Some(_)) if the process already exited.
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process exited — clean up the handle.
                    *guard = None;
                    ServerStatus::Error(format!("llama-server exited with status {}", status))
                }
                Ok(None) => ServerStatus::Running,
                Err(e) => ServerStatus::Error(format!("try_wait error: {}", e)),
            }
        }
    }
}
