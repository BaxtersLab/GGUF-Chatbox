Changelog — Recent edits (short)

Summary:
- Fixed Windows orphaned-process issues and spawn supervision, reordered proxy bind before server spawn, and resolved multiple compilation/runtime bugs so the workspace builds and tests pass.

Files changed (high level):
- src-tauri/src/main.rs: Fixed duplicate imports, corrected lifetime bug when parsing HTTP headers, adjusted `terminate_spawned_children` call to pass the `Mutex` reference, iterated `spawned_pids` by reference to avoid move errors, and reordered proxy bind/start logic.
- crates/adaptive_llama/Cargo.toml: Added Windows-only dependencies/features (`windows-sys`, `once_cell`) required for JobObject helper.
- crates/adaptive_llama/src/windows_job.rs: Added a minimal Windows JobObject helper (create job, set KILL_ON_JOB_CLOSE, assign child processes).
- crates/adaptive_llama/src/process.rs: Updated process spawn to avoid detached process group flags on Windows and assign spawned children to the JobObject when available.
- Launcher & cleanup scripts: `launcher_supervisor.py` and `cleanup-orphans.ps1` were added during iteration and later removed after integrating supervision into the app.

Why these changes:
- Prevent race where proxy bind failure left spawned model workers orphaned (Windows port bind error 10048).
- Ensure child processes are supervised (JobObject) so OS kills them on parent exit.
- Remove detached process-group flags to keep parent/child ownership on Windows.
- Fix Rust borrow/lifetime and ownership errors discovered during rebuild.

Test & verification:
- Ran `cargo build --workspace -v` and `cargo test --workspace` successfully — all tests passed (34 tests across crates).
- Confirmed Tauri app crate (`gguf-chatbox-app`) builds cleanly after fixes.

Notes / next steps:
- There are a few non-fatal warnings (deprecated `base64::decode`, unused variables). I can clean them up in a follow-up if you want.
- If you want a formal git-style patch or PR summary, I can generate diffs of the exact edits.

If you want the full detailed diffs for each modified file, say "generate diffs" and I'll produce them.