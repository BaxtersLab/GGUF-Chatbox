use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::registry::ToolHandler;
use crate::types::ToolError;

/// Tool: vscodium_workspace
///
/// The GGUF-Chatbox side of the Agent-6 tool bridge. A6's model emits a normal
/// OpenAI tool call; the proxy's tool loop dispatches it here; this handler
/// hands the operation to A6's headless VSCodium (via the A6 VSCodium Tools
/// extension) and returns what VSCodium did. The extension is the "hands"; this
/// is the "connection" — and it lives in GGUF Chatbox (not the Master Widget) so
/// A6's tools work wherever GGUF Chatbox runs.
///
/// Channel: a file-drop bridge (default `~/.gguf-chatbox/a6_tools`, overridable
/// via the `A6_TOOLS_DIR` env var). A command JSON is written to `inbox/`; the
/// extension executes it and writes the result to `outbox/<id>.json`. The op set
/// mirrors the extension's PROTOCOL.md exactly.
///
/// The VSCodium-side half of this bridge (the extension that reads the inbox and
/// runs the ops via VSCodium's API) lives in this repo at `vscodium-extension/`.
///
/// Args: { "op": "create_folder" | "create_file" | "read_file" | "list_dir"
///          | "add_workspace_folder" | "remove_workspace_folder"
///          | "list_workspace_folders",
///         "path": "...", "content": "..." }
pub struct VscodiumTool {
    bridge_dir: PathBuf,
    timeout: Duration,
    poll: Duration,
}

/// The operations the VSCodium extension understands (PROTOCOL.md). Kept here so
/// a bad op is rejected before a doomed command is ever written to the inbox.
const OPS: &[&str] = &[
    "create_folder",
    "create_file",
    "read_file",
    "list_dir",
    "add_workspace_folder",
    "remove_workspace_folder",
    "list_workspace_folders",
];

fn default_bridge_dir() -> PathBuf {
    if let Ok(d) = std::env::var("A6_TOOLS_DIR") {
        let d = d.trim().to_string();
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".gguf-chatbox")
        .join("a6_tools")
}

impl VscodiumTool {
    /// Default: bridge under `~/.gguf-chatbox/a6_tools` (or `$A6_TOOLS_DIR`),
    /// 30s round-trip timeout.
    pub fn new() -> Self {
        VscodiumTool {
            bridge_dir: default_bridge_dir(),
            timeout: Duration::from_secs(30),
            poll: Duration::from_millis(100),
        }
    }

    /// Explicit bridge dir + timeout — used by tests.
    pub fn with_config(bridge_dir: PathBuf, timeout: Duration) -> Self {
        VscodiumTool { bridge_dir, timeout, poll: Duration::from_millis(20) }
    }

    /// Run one operation through the extension and return its result payload as a
    /// JSON string (the `result` object on success). Blocks up to `timeout`.
    fn run(&self, op: &str, mut args: Value) -> Result<String, ToolError> {
        if !OPS.contains(&op) {
            return Err(ToolError(format!(
                "unknown vscodium op '{op}' (expected one of: {})",
                OPS.join(", ")
            )));
        }
        // The extension's command shape is { id, op, args }. Forward every arg
        // except the op selector, so per-op fields (path, content, future ones)
        // pass through without this handler having to know them all.
        if let Some(obj) = args.as_object_mut() {
            obj.remove("op");
        }

        let inbox = self.bridge_dir.join("inbox");
        let outbox = self.bridge_dir.join("outbox");
        std::fs::create_dir_all(&inbox)
            .map_err(|e| ToolError(format!("cannot create bridge inbox: {e}")))?;

        let id = next_id();
        let command = json!({ "id": id, "op": op, "args": args });

        // Atomic publish: write .tmp then rename, so the extension never reads a
        // half-written command.
        let final_path = inbox.join(format!("{id}.json"));
        let tmp_path = inbox.join(format!("{id}.json.tmp"));
        std::fs::write(&tmp_path, command.to_string())
            .map_err(|e| ToolError(format!("cannot write command: {e}")))?;
        std::fs::rename(&tmp_path, &final_path)
            .map_err(|e| ToolError(format!("cannot publish command: {e}")))?;

        // Poll the outbox for <id>.json until it appears or we time out.
        let result_path = outbox.join(format!("{id}.json"));
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Ok(text) = std::fs::read_to_string(&result_path) {
                let _ = std::fs::remove_file(&result_path); // don't accumulate
                return parse_result(&text);
            }
            if Instant::now() >= deadline {
                return Err(ToolError(format!(
                    "timed out after {:?} waiting for VSCodium to run '{op}' — is A6's VSCodium \
                     running with the A6 tools extension, watching {}?",
                    self.timeout,
                    self.bridge_dir.display()
                )));
            }
            std::thread::sleep(self.poll);
        }
    }
}

/// Interpret the extension's result file: `{id, op, ok, result|error}`.
fn parse_result(text: &str) -> Result<String, ToolError> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| ToolError(format!("bad result from VSCodium: {e}")))?;
    if v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) {
        // Hand the model the result payload (or a bare ok when there is none).
        Ok(match v.get("result") {
            Some(r) => r.to_string(),
            None => json!({ "ok": true }).to_string(),
        })
    } else {
        let msg = v.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error");
        Err(ToolError(format!("VSCodium: {msg}")))
    }
}

/// Process-unique, filesystem-safe command id.
fn next_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("gguf-{ms}-{n}")
}

impl Default for VscodiumTool {
    fn default() -> Self { Self::new() }
}

impl ToolHandler for VscodiumTool {
    fn name(&self) -> &str { "vscodium_workspace" }

    fn description(&self) -> &str {
        "Act on Agent 6's VSCodium workspace through the VSCodium tools extension: \
         create/read/write files and folders, and manage workspace folders. File \
         operations are contained to the workspace."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": OPS,
                    "description": "The workspace operation to perform."
                },
                "path": {
                    "type": "string",
                    "description": "Target path, relative to the workspace (or absolute). \
                                    Required for all ops except list_workspace_folders."
                },
                "content": {
                    "type": "string",
                    "description": "File content (for create_file)."
                }
            },
            "required": ["op"]
        })
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let op = args.get("op").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError("'op' argument is required".to_string()))?
            .to_string();
        self.run(&op, args)
    }
}
