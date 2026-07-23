// GGUF Chatbox — tool_belt crate

pub mod types;
pub mod registry;
pub mod system_info;
pub mod file_ops;
pub mod vscodium_ops;

pub use types::{ToolError, ToolDescriptor};
pub use registry::{ToolHandler, ToolRegistry};
pub use system_info::SystemInfoTool;
pub use file_ops::FileOpsTool;
pub use vscodium_ops::VscodiumTool;

/// Build a default ToolRegistry with system_info, file_ops, and the VSCodium
/// workspace bridge (Agent 6's hands) pre-registered.
pub fn default_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Box::new(SystemInfoTool));
    r.register(Box::new(FileOpsTool::new()));
    r.register(Box::new(VscodiumTool::new()));
    r
}
