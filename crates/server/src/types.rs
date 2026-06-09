use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Configuration passed to `start_server`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub model_path: PathBuf,
    pub context_length: u32,
    pub threads: u32,
    /// Multimodal projector file (.gguf).  Required for vision-capable models
    /// (Phi-4 multimodal, LLaVA, MiniCPM-V, etc.).  `None` for text-only models.
    #[serde(default)]
    pub mmproj_path: Option<PathBuf>,
    /// Optional temperature override from the active app profile.
    #[serde(default)]
    pub temperature_override: Option<f32>,
    /// Optional n_predict (max tokens) override from the active app profile.
    /// Negative value (-1) means unlimited.
    #[serde(default)]
    pub n_predict_override: Option<i32>,
    /// Optional hard cap on context length from the active app profile.
    /// When set, further clamps the auto-calculated context to this value.
    #[serde(default)]
    pub ctx_cap_override: Option<u32>,
}

/// Current state of the llama-server process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerStatus {
    /// No server process exists.
    Stopped,
    /// Process is running but /health not yet returning "ok".
    Starting,
    /// Process is running and /health returns "ok".
    Running,
    /// Process exited unexpectedly.
    Crashed,
    /// Process exists but health check could not connect.
    Down,
}

/// Trait that proxy.rs uses to dispatch tool calls.
/// Implemented by the tool_belt crate — the server crate only depends
/// on this trait, not on tool_belt directly.
pub trait ToolDispatcher {
    fn dispatch(&self, name: &str, args: serde_json::Value) -> Result<String, String>;
}

/// A no-op dispatcher used when no tool belt is wired.
pub struct NoopDispatcher;
impl ToolDispatcher for NoopDispatcher {
    fn dispatch(&self, name: &str, _args: serde_json::Value) -> Result<String, String> {
        Err(format!("no tool registered: {name}"))
    }
}
