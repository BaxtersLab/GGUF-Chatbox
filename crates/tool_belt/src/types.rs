use serde::Serialize;

/// Error type returned by tool execution.
#[derive(Debug)]
pub struct ToolError(pub String);

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ToolError {
    fn from(s: String) -> Self { ToolError(s) }
}

impl From<&str> for ToolError {
    fn from(s: &str) -> Self { ToolError(s.to_string()) }
}

/// Describes a registered tool for MCP discovery or schema listing.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}
