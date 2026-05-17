// GGUF Chatbox — adaptive_llama crate (cloned from AiSmartGuy model_loader)

pub mod errors;
pub mod types;
pub mod state_machine;
pub mod gpu_mapper;
pub mod command_builder;
pub mod timeout;
pub mod process;
pub mod loader;
pub mod inferencer;
pub mod llama_detect;
pub mod log_callback;
pub mod server_bridge;
pub mod card_reader;

pub use errors::{ModelError, LoaderResult};
pub use types::{ModelConfig, ModelInstance, ModelState, InferenceRequest, InferenceMode, ChatMessage, SamplingConfig};
pub use gpu_mapper::query_vram_mb;
pub use command_builder::{build_oneshot_command, build_chat_command};
pub use loader::{load_model, unload_model};
pub use inferencer::{run_inference, run_chat_inference, TokenCallback, DEFAULT_TIMEOUT};
pub use timeout::inference_timeout_for;
pub use llama_detect::{auto_gpu_layers, detect_llama, detect_llama_server, gguf_block_count, gguf_context_length, llama_install_dir, llama_local_path, llama_server_local_path, max_context_for_vram, resolve_llama_path, resolve_llama_server_path, LLAMA_BIN, LLAMA_SERVER_BIN};
pub use log_callback::{set_log_callback, clear_log_callback};
pub use server_bridge::{start_server, stop_server, health_check, ServerConfig, ServerStatus};
pub use card_reader::{ModelCard, CardParams, load_or_build_card, load_cached_card, save_card, merge_hf_readme, gguf_read_string_key};

#[cfg(windows)]
pub mod windows_job;
