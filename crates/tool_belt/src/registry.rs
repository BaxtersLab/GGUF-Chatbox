use std::collections::HashMap;
use serde_json::Value;

use crate::types::{ToolDescriptor, ToolError};

/// Implement this trait to add a new tool to the registry.
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn execute(&self, args: Value) -> Result<String, ToolError>;
}

/// Central registry — stores all registered tools and dispatches calls by name.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    /// Register a tool. If a tool with the same name already exists it is replaced.
    pub fn register(&mut self, tool: Box<dyn ToolHandler>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Dispatch a tool call by name, forwarding `args` to the handler.
    pub fn dispatch(&self, name: &str, args: Value) -> Result<String, ToolError> {
        match self.tools.get(name) {
            Some(tool) => tool.execute(args),
            None => Err(ToolError(format!("unknown tool: {name}"))),
        }
    }

    /// List all registered tools (used by MCP discovery and proxy schema injection).
    pub fn list_tools(&self) -> Vec<ToolDescriptor> {
        self.tools.values().map(|t| ToolDescriptor {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters_schema: t.parameters_schema(),
        }).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}
