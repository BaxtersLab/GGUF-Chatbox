use serde::{Deserialize, Serialize};

/// Condensed source material to echo back at milestones.
#[derive(Debug, Clone)]
pub struct EchoSource {
    pub system_prompt: String,
    pub key_facts: Vec<String>,
}

/// Configuration controlling which milestones fire and how much text to echo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EchoConfig {
    /// Token-usage fractions at which to inject an echo (e.g. 0.25, 0.50, 0.75).
    pub milestones: Vec<f32>,
    /// Maximum fraction of total context to spend on a single echo injection.
    pub max_echo_pct: f32,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self {
            milestones: vec![0.25, 0.50, 0.75],
            max_echo_pct: 0.01,
        }
    }
}

/// Returned when a milestone threshold is crossed.
#[derive(Debug, Clone)]
pub struct MilestoneHit {
    pub threshold: f32,
    pub echo_text: String,
}

/// A single chat turn — defined locally so context_echo has no dep on adaptive_llama.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}
