// GGUF Chatbox — context_echo crate

pub mod types;
pub mod echo;

pub use types::{ChatMessage, EchoConfig, EchoSource, MilestoneHit};
pub use echo::{auto_scale_echo, capture_echo_source, check_milestone, inject_echo};
