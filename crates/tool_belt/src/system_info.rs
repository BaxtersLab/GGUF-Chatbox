use serde_json::{json, Value};

use adaptive_llama::{query_vram_mb, gguf_block_count, gguf_context_length};

use crate::registry::ToolHandler;
use crate::types::ToolError;

/// Tool: system_info
///
/// Returns GPU stats, model info, or a list of available models.
///
/// Args: { "query": "gpu_stats" | "model_info" | "list_models" }
///       For "model_info": also accepts { "model_path": "..." }
pub struct SystemInfoTool;

impl ToolHandler for SystemInfoTool {
    fn name(&self) -> &str { "system_info" }

    fn description(&self) -> &str {
        "Returns GPU stats, loaded model information, or a list of available local models."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "enum": ["gpu_stats", "model_info", "list_models"],
                    "description": "What to query."
                },
                "model_path": {
                    "type": "string",
                    "description": "Path to a .gguf file (used with model_info query)."
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let query = args["query"].as_str().unwrap_or("gpu_stats");
        match query {
            "gpu_stats" => {
                let vram_mb = query_vram_mb();
                let result = json!({
                    "vram_total_mb": vram_mb,
                    "note": "VRAM free approximation via nvidia-smi or fallback estimation."
                });
                Ok(result.to_string())
            }
            "model_info" => {
                let model_path_str = args["model_path"].as_str()
                    .ok_or_else(|| ToolError("model_path is required for model_info".to_string()))?;
                let path = std::path::PathBuf::from(model_path_str);
                let ctx   = gguf_context_length(&path).unwrap_or(0);
                let layers = gguf_block_count(&path).unwrap_or(0);
                let size_mb = std::fs::metadata(&path)
                    .map(|m| m.len() / (1024 * 1024))
                    .unwrap_or(0);
                let result = json!({
                    "path": model_path_str,
                    "context_length": ctx,
                    "layers": layers,
                    "size_mb": size_mb,
                });
                Ok(result.to_string())
            }
            "list_models" => {
                let home = std::env::var("USERPROFILE")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                let models_dir = home.join(".gguf-chatbox").join("models");
                let mut models: Vec<Value> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&models_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) != Some("gguf") {
                            continue;
                        }
                        let name = p.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        let size_mb = std::fs::metadata(&p)
                            .map(|m| m.len() / (1024 * 1024))
                            .unwrap_or(0);
                        models.push(json!({ "name": name, "size_mb": size_mb }));
                    }
                }
                Ok(json!({ "models": models }).to_string())
            }
            _ => Err(ToolError(format!("unknown query: {query}"))),
        }
    }
}
