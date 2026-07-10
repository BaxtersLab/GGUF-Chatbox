//! ToolRegistry contract: register/dispatch/replace/list, plus the default
//! registry shipped to the proxy and MCP discovery.

use serde_json::{json, Value};
use tool_belt::{default_registry, ToolError, ToolHandler, ToolRegistry};

struct EchoTool {
    reply: &'static str,
}

impl ToolHandler for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "echoes its input"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    fn execute(&self, args: Value) -> Result<String, ToolError> {
        Ok(format!("{}:{}", self.reply, args["text"].as_str().unwrap_or("")))
    }
}

#[test]
fn register_and_dispatch() {
    let mut r = ToolRegistry::new();
    r.register(Box::new(EchoTool { reply: "v1" }));
    let out = r.dispatch("echo", json!({"text": "hi"})).unwrap();
    assert_eq!(out, "v1:hi");
}

#[test]
fn unknown_tool_is_an_error() {
    let r = ToolRegistry::new();
    let err = r.dispatch("nope", json!({})).unwrap_err();
    assert!(err.to_string().contains("unknown tool"), "got: {err}");
}

#[test]
fn reregistering_replaces_the_handler() {
    let mut r = ToolRegistry::new();
    r.register(Box::new(EchoTool { reply: "v1" }));
    r.register(Box::new(EchoTool { reply: "v2" }));
    let out = r.dispatch("echo", json!({"text": "x"})).unwrap();
    assert_eq!(out, "v2:x");
    assert_eq!(r.list_tools().len(), 1, "replacement must not duplicate the entry");
}

#[test]
fn default_registry_ships_system_info_and_file_ops() {
    let r = default_registry();
    let mut names: Vec<String> = r.list_tools().into_iter().map(|t| t.name).collect();
    names.sort();
    assert_eq!(names, vec!["file_ops".to_string(), "system_info".to_string()]);
}
