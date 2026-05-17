use serde_json::{json, Value};

use crate::registry::ToolHandler;
use crate::types::ToolError;

/// Tool: file_ops
///
/// Read, write, or list files — sandboxed to `~/.gguf-chatbox/` by default.
/// Additional allowed directories can be injected at construction time.
///
/// Args: { "op": "read" | "write" | "list", "path": "..." }
///       For "write": also accepts { "content": "..." }
pub struct FileOpsTool {
    /// The root sandbox directory.  All paths are resolved relative to it
    /// (or checked to be inside it for absolute paths).
    sandbox_root: std::path::PathBuf,
}

impl FileOpsTool {
    /// Create a FileOpsTool sandboxed to `~/.gguf-chatbox/`.
    pub fn new() -> Self {
        let root = std::env::var("USERPROFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(".gguf-chatbox");
        FileOpsTool { sandbox_root: root }
    }

    /// Create a FileOpsTool with an explicit sandbox root.
    pub fn with_root(root: std::path::PathBuf) -> Self {
        FileOpsTool { sandbox_root: root }
    }

    /// Resolve and validate a path against the sandbox root.
    ///
    /// Rejects any path that would escape the sandbox via `..` or absolute prefix.
    fn resolve_safe(&self, raw: &str) -> Result<std::path::PathBuf, ToolError> {
        // Build candidate: join relative paths, or validate absolute ones.
        let candidate = if std::path::Path::new(raw).is_absolute() {
            std::path::PathBuf::from(raw)
        } else {
            self.sandbox_root.join(raw)
        };

        // Canonicalize without requiring the path to exist (normalise only).
        // We manually check the prefix instead to avoid requiring file existence.
        let normalised = normalise_path(&candidate);

        // Ensure normalised path starts with the sandbox root.
        let root_normalised = normalise_path(&self.sandbox_root);
        if !normalised.starts_with(&root_normalised) {
            return Err(ToolError(format!(
                "path '{}' is outside the allowed sandbox",
                raw
            )));
        }

        Ok(normalised)
    }
}

/// Normalise path separators and resolve `..` components without I/O.
fn normalise_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut result = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => { result.pop(); }
            std::path::Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

impl Default for FileOpsTool {
    fn default() -> Self { Self::new() }
}

impl ToolHandler for FileOpsTool {
    fn name(&self) -> &str { "file_ops" }

    fn description(&self) -> &str {
        "Read, write, or list files inside the ~/.gguf-chatbox/ sandbox."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["read", "write", "list"],
                    "description": "Operation to perform."
                },
                "path": {
                    "type": "string",
                    "description": "Relative path within the sandbox (or absolute within allowed root)."
                },
                "content": {
                    "type": "string",
                    "description": "Content to write (required for 'write' op)."
                }
            },
            "required": ["op", "path"]
        })
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let op   = args["op"].as_str().unwrap_or("");
        let path = args["path"].as_str()
            .ok_or_else(|| ToolError("'path' argument is required".to_string()))?;

        let safe_path = self.resolve_safe(path)?;

        match op {
            "read" => {
                let content = std::fs::read_to_string(&safe_path)
                    .map_err(|e| ToolError(format!("read failed: {e}")))?;
                Ok(json!({ "path": path, "content": content }).to_string())
            }
            "write" => {
                let content = args["content"].as_str()
                    .ok_or_else(|| ToolError("'content' argument is required for write".to_string()))?;
                if let Some(parent) = safe_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| ToolError(format!("mkdir failed: {e}")))?;
                }
                std::fs::write(&safe_path, content)
                    .map_err(|e| ToolError(format!("write failed: {e}")))?;
                Ok(json!({ "path": path, "written": content.len() }).to_string())
            }
            "list" => {
                let entries: Vec<Value> = std::fs::read_dir(&safe_path)
                    .map_err(|e| ToolError(format!("list failed: {e}")))?
                    .flatten()
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        json!({ "name": name, "is_dir": is_dir })
                    })
                    .collect();
                Ok(json!({ "path": path, "entries": entries }).to_string())
            }
            _ => Err(ToolError(format!("unknown op: '{op}'"))),
        }
    }
}
