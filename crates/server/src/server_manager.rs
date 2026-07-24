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

// ── Orphan protection ────────────────────────────────────────────────────────
// llama-server holds the model in VRAM (GBs). If this app is force-killed or
// crashes, Rust never runs the Drop/kill path, so the child SURVIVES as a
// headless zombie squatting :8081 and its VRAM — invisible, and it blocks the
// next start. Two defences:
//   1. a Windows Job Object with KILL_ON_JOB_CLOSE: the OS kills the child when
//      this process dies for ANY reason (structural — no cooperation needed);
//   2. a reaper that clears leftovers before each start (covers orphans from a
//      crash that predates this fix, or from an older build).
// Raw FFI keeps the crate dependency-free (matches its tiny_http-only style).

#[cfg(windows)]
mod job {
    use std::ffi::c_void;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(job: *mut c_void, class: u32, info: *mut c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    /// Create a job whose members are killed when the last handle to it closes —
    /// i.e. when this process exits, however it exits. Handle returned as isize
    /// so it can live in a static (raw pointers aren't Send).
    pub fn create_kill_on_close() -> Option<isize> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info = ExtendedLimitInformation::default();
            info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            Some(h as isize)
        }
    }

    /// Put a process (by raw HANDLE) under the job. Best-effort.
    pub fn assign(job: isize, process: isize) -> bool {
        unsafe { AssignProcessToJobObject(job as *mut c_void, process as *mut c_void) != 0 }
    }

    /// Close a job handle — kills its members when this was the last handle.
    /// Used by tests to prove the kill-on-close contract.
    pub fn close(job: isize) {
        unsafe {
            CloseHandle(job as *mut c_void);
        }
    }
}

#[cfg(windows)]
static JOB: Mutex<Option<isize>> = Mutex::new(None);

/// Adopt the spawned child into the process-lifetime job so it cannot outlive us.
#[cfg(windows)]
fn adopt_into_job(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    let mut guard = match JOB.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_none() {
        *guard = job::create_kill_on_close();
    }
    if let Some(j) = *guard {
        let _ = job::assign(j, child.as_raw_handle() as isize);
    }
}

#[cfg(not(windows))]
fn adopt_into_job(_child: &Child) {}

/// Kill any llama-server left over from a previous run before starting a new one,
/// so at most one ever holds the model/VRAM. Best-effort and quiet.
fn reap_orphan_servers() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "llama-server.exe"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("pkill")
            .args(["-f", "llama-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

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

    // Clear any UNTRACKED leftover (orphaned by a crash/force-kill of a previous
    // run) so exactly one llama-server — and one copy of the model in VRAM —
    // exists after this start.
    reap_orphan_servers();

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
    // Bind the child's lifetime to ours: if this process is force-killed or
    // crashes, the OS tears the child down too (no headless VRAM zombie).
    adopt_into_job(&child);
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

    /// The orphan guarantee, proven on a stand-in child: a process placed in the
    /// kill-on-close job MUST die when the last job handle closes — which is what
    /// the OS does for us when this app is force-killed or crashes. Without this,
    /// llama-server survives as a headless zombie holding GBs of VRAM.
    #[cfg(windows)]
    #[test]
    fn job_kills_its_child_when_the_job_handle_closes() {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;

        let job = super::job::create_kill_on_close().expect("job object created");

        // A stand-in for llama-server: something that would otherwise run on.
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .expect("spawned the stand-in child");

        assert!(
            super::job::assign(job, child.as_raw_handle() as isize),
            "child should join the job"
        );
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "child should still be alive before the job closes"
        );

        // Closing the last handle is exactly what process death does for us.
        super::job::close(job);

        let mut died = false;
        for _ in 0..50 {
            if child.try_wait().expect("try_wait").is_some() {
                died = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !died {
            let _ = child.kill(); // don't leak the stand-in if the guarantee failed
        }
        assert!(died, "closing the job MUST kill its child (orphan protection)");
    }
}
