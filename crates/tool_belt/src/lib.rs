// GGUF Chatbox — tool_belt crate

pub mod types;
pub mod registry;
pub mod system_info;
pub mod file_ops;

pub use types::{ToolError, ToolDescriptor};
pub use registry::{ToolHandler, ToolRegistry};
pub use system_info::SystemInfoTool;
pub use file_ops::FileOpsTool;

/// Build a default ToolRegistry with system_info and file_ops pre-registered.
pub fn default_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Box::new(SystemInfoTool));
    r.register(Box::new(FileOpsTool::new()));
    r
}
