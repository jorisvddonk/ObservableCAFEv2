use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool call parsed from LLM output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Full method name, e.g. "sheetbot.list_tasks".
    pub name: String,
    /// Parameters as a JSON object.
    pub parameters: Value,
}

/// Result produced by executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Matches the tool call name.
    pub name: String,
    /// The output/return value of the tool.
    pub output: Value,
    /// Set if the tool execution failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tool definition used in agent config annotations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the parameters object.
    pub parameters: Value,
    /// "rpc" for bus-dispatched tools, "builtin" for in-process.
    #[serde(default = "default_tool_type")]
    pub tool_type: String,
}

fn default_tool_type() -> String {
    "rpc".into()
}
