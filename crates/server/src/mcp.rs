/// MCP (Model Context Protocol) server — stub implementation.
///
/// Future: expose tool_belt tools via MCP so Continue/VSCodium
/// can discover and invoke them directly on port 8082.
/// Currently a placeholder — no functionality yet.

/// Start the MCP server (no-op in the current build).
pub fn start_mcp_server() {
    // TODO: implement MCP endpoint on port 8082
}

/// Stop the MCP server (no-op in the current build).
pub fn stop_mcp_server() {
    // TODO: graceful shutdown
}
