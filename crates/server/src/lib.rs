// GGUF Chatbox — server crate

pub mod types;
pub mod server_manager;
pub mod proxy;
pub mod mcp;

pub use types::{ServerConfig, ServerStatus, ToolDispatcher, NoopDispatcher};
pub use server_manager::{start_server, stop_server, health_check, model_name_from_path};
pub use proxy::{start_proxy, start_proxy_on};
pub use mcp::{start_mcp_server, stop_mcp_server};
