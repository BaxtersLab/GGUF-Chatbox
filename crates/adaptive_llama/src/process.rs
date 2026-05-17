use std::process::Stdio;
use crate::errors::{LoaderResult, ModelError};
use crate::types::ModelInstance;

/// Spawns `cmd` as a subprocess. Returns the live `Child` handle.
///
/// The caller is responsible for waiting on the child (via `timeout::enforce_timeout`
/// or `child.wait()`).
pub fn spawn_process(_instance: &ModelInstance, cmd: std::process::Command) -> LoaderResult<std::process::Child> {
    let mut cmd = cmd;

    // Explicit stdin: llama-cli reads from -f file, never stdin.
    cmd.stdin(Stdio::null());
    // Capture stderr so we can log it (don't suppress — we need diagnostics).
    cmd.stderr(Stdio::piped());

    // On Windows, hide the console window that llama-cli would otherwise open.
    // NOTE: do NOT create detached process groups here. Detached/new process
    // groups (`CREATE_NEW_PROCESS_GROUP`) weaken parent ownership on Windows
    // and make child processes less likely to be cleaned up if the parent
    // exits during startup. We keep only `CREATE_NO_WINDOW` here and
    // plan to assign spawned children to a Job Object in a follow-up patch.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // Log the full command line to the live terminal for debugging.
    let args: Vec<String> = std::iter::once(cmd.get_program().to_string_lossy().into_owned())
        .chain(cmd.get_args().map(|a| a.to_string_lossy().into_owned()))
        .collect();
    let cmdline = args.join(" ");
    eprintln!("[model_loader][INFO] exec: {}", cmdline);
    crate::log_callback::emit_log_line(&format!("[exec] {}", cmdline));

    // TODO (windows): in a follow-up change, create a small `windows_job`
    // helper that creates a Win32 Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
    // and assigns the spawned child to that job so the OS will terminate
    // the child if the parent crashes. That helper should be invoked here
    // immediately after successful `spawn()` and guarded by `#[cfg(windows)]`.
    let child = cmd.spawn().map_err(|e| {
        ModelError::LoadFailure(format!(
            "failed to spawn llama process: {}",
            e
        ))
    })?;

    // On Windows, attempt to assign the spawned child to a Job Object so
    // that if the parent exits unexpectedly the OS will terminate the
    // child. This is a conservative safety net against orphaned model
    // processes after a bind failure or crash. The implementation is a
    // small helper in `windows_job.rs` and is guarded by cfg(windows).
    #[cfg(windows)]
    {
        if let Err(e) = crate::windows_job::windows_job::assign_child_to_job(&child) {
            eprintln!("[model_loader][WARN] failed to assign child to job: {}", e);
        }
    }

    Ok(child)
}

/// Kills a running subprocess unconditionally. Logs any kill failure but does
/// not propagate it as an error — unloading must always succeed.
pub fn kill_process(child: &mut std::process::Child) {
    if let Err(e) = child.kill() {
        eprintln!("[model_loader][WARN] kill failed (process may have already exited): {}", e);
    }
    // Reap the zombie so we don't leave orphaned processes.
    let _ = child.wait();
}
