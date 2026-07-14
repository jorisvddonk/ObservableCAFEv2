use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use cafe_sdk::{keys, Chunk, ServerMessage, ToolCall, ToolResult};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Top-level config file.
#[derive(Debug, Deserialize)]
struct McpConfig {
    #[serde(default)]
    server: Vec<McpServerConfig>,
}

/// An MCP server definition from mcp-servers.toml.
#[derive(Debug, Clone, Deserialize)]
struct McpServerConfig {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

/// A running MCP server process with its stdio channels.
struct McpServerInstance {
    _process: Child,
    writer: tokio::io::BufWriter<tokio::process::ChildStdin>,
    reader: tokio::sync::mpsc::Receiver<String>,
}

/// Registry of MCP servers and their tools.
struct Registry {
    servers: HashMap<String, Arc<Mutex<McpServerInstance>>>,
    /// Tool name → server name mapping
    tool_map: HashMap<String, String>,
}

/// Owns the JSON-RPC call-id sequence used when invoking MCP tools.
///
/// Ids 1, 2 and 3 are reserved for the per-server `initialize` and
/// `tools/list` handshakes, so tool calls start at id 4 and increment
/// monotonically across the lifetime of the client.
pub(crate) struct McpClient {
    next_call_id: AtomicU64,
}

impl McpClient {
    pub(crate) fn new() -> Self {
        // Reserve 1/2/3 for initialize and tools/list handshakes.
        Self {
            next_call_id: AtomicU64::new(4),
        }
    }

    /// Allocate the next strictly-increasing call id.
    pub(crate) fn next_call_id(&self) -> u64 {
        // FIX A: the counter lives on the client and persists across calls, so
        // every tool call gets a unique id (previously it was reset to 3 inside
        // the per-call lock, making every call use id 4 — so a stale id-4
        // response left after a timeout could be consumed as the next result).
        self.next_call_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Invoke a tool on an already-locked MCP server instance and return the
    /// raw JSON-RPC response.
    pub(crate) async fn call_tool(
        &self,
        instance: &mut McpServerInstance,
        tool_call: &ToolCall,
    ) -> Result<serde_json::Value> {
        let call_id = self.next_call_id();
        send_jsonrpc(
            &mut instance.writer,
            call_id,
            "tools/call",
            serde_json::json!({
                "name": tool_call.name,
                "arguments": tool_call.parameters,
            }),
        )
        .await?;
        read_response(&mut instance.reader, call_id).await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("CAFE_MCP_SERVERS")
        .unwrap_or_else(|_| "mcp-servers.toml".into());

    let config_text = tokio::fs::read_to_string(&config_path).await?;
    let cfg: McpConfig = toml::from_str(&config_text)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", config_path, e))?;
    let servers = cfg.server;

    info!("cafe-mcp-client: {} server(s) configured", servers.len());

    // Start each server and discover tools
    let mut registry = Registry {
        servers: HashMap::new(),
        tool_map: HashMap::new(),
    };

    for cfg in &servers {
        let mut instance = start_server(cfg).await?;

        // Initialize the server
        send_jsonrpc(&mut instance.writer, 1, "initialize", serde_json::json!({})).await?;
        let _init_resp = read_response(&mut instance.reader, 1).await?;

        // Discover tools
        send_jsonrpc(&mut instance.writer, 2, "tools/list", serde_json::json!({})).await?;
        let tools_resp = read_response(&mut instance.reader, 2).await?;

        if let Some(tools) = tools_resp["result"]["tools"].as_array() {
            for tool in tools {
                if let Some(name) = tool["name"].as_str() {
                    info!("  mcp server '{}': tool '{}'", cfg.name, name);
                    registry.tool_map.insert(name.to_string(), cfg.name.clone());
                }
            }
        }

        registry.servers.insert(cfg.name.clone(), Arc::new(Mutex::new(instance)));
    }

    info!(
        "cafe-mcp-client: {} tools from {} server(s) registered",
        registry.tool_map.len(),
        registry.servers.len()
    );

    let registry = Arc::new(registry);
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    let client = cafe_sdk::bus::BusClient::unix(&socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let reg = registry.clone();
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c, reg).await {
                    warn!("cafe-mcp-client: session error: {}", e);
                }
            });
        }
    }

    Ok(())
}

async fn run_session(
    session_id: String,
    client: cafe_sdk::bus::BusClient,
    registry: Arc<Registry>,
) -> Result<()> {
    let mut rx = client.subscribe(&session_id).await?;
    let mcp = McpClient::new();

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        // Only handle tool.call chunks with provider:mcp
        let Some(tool_call) = chunk.as_tool_call() else { continue; };
        if tool_call.provider.as_deref() != Some("mcp") {
            continue;
        }

        let server_name = match registry.tool_map.get(&tool_call.name) {
            Some(s) => s.clone(),
            None => {
                warn!("cafe-mcp-client: no server registered for tool '{}'", tool_call.name);
                continue;
            }
        };

        info!(
            "cafe-mcp-client: executing '{}' via server '{}'",
            tool_call.name, server_name
        );

        let server = match registry.servers.get(&server_name) {
            Some(s) => s.clone(),
            None => continue,
        };

        let result = {
            let mut instance = server.lock().await;
            mcp.call_tool(&mut instance, &tool_call).await
        };

        match result {
            Ok(resp) => {
                let tool_result = parse_tool_result(&resp, &tool_call.name);

                let result_chunk = Chunk::new_null("com.nominal.cafe-mcp-client")
                    .with_annotation(keys::CAFE_TOOL_RESULT, &tool_result)
                    .as_transient()
                    .with_retain(60);
                let _ = client.publish(&session_id, result_chunk).await;

                // Also publish a human-readable text chunk
                let text = if let Some(err) = &tool_result.error {
                    format!(
                        "MCP tool failed: {}\n```\n{}\n```",
                        tool_call.name, err
                    )
                } else {
                    let output_text = serde_json::to_string_pretty(&tool_result.output)
                        .unwrap_or_else(|_| "{}".into());
                    format!(
                        "MCP tool completed: {}\n```\n{}\n```",
                        tool_call.name, output_text
                    )
                };
                let text_chunk = Chunk::new_text(&text, "com.nominal.cafe-mcp-client")
                    .with_annotation(keys::CHAT_ROLE, "assistant");
                let _ = client.publish(&session_id, text_chunk).await;
            }
            Err(e) => {
                warn!("cafe-mcp-client: MCP call failed: {}", e);
                let tool_result = ToolResult {
                    name: tool_call.name.clone(),
                    output: serde_json::Value::Null,
                    error: Some(e.to_string()),
                    provider: Some("mcp".into()),
                };
                let result_chunk = Chunk::new_null("com.nominal.cafe-mcp-client")
                    .with_annotation(keys::CAFE_TOOL_RESULT, &tool_result)
                    .as_transient()
                    .with_retain(60);
                let _ = client.publish(&session_id, result_chunk).await;
            }
        }
    }

    Ok(())
}

/// Parse an MCP JSON-RPC tool response into a `ToolResult`.
///
/// FIX B: surface both top-level JSON-RPC `error` and MCP `isError:true`
/// as failures, and surface non-text content faithfully instead of
/// collapsing it to `"{}"`.
pub(crate) fn parse_tool_result(resp: &serde_json::Value, name: &str) -> ToolResult {
    // JSON-RPC level error.
    if let Some(error) = resp.get("error") {
        let msg = match error {
            serde_json::Value::String(s) => s.clone(),
            other => serde_json::to_string(other)
                .unwrap_or_else(|_| "MCP JSON-RPC error".to_string()),
        };
        return ToolResult {
            name: name.to_string(),
            output: serde_json::Value::Null,
            error: Some(msg),
            provider: Some("mcp".into()),
        };
    }

    let result = resp.get("result").cloned().unwrap_or(serde_json::Value::Null);

    // MCP-level isError.
    if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        let detail = extract_text(&result).unwrap_or_default();
        return ToolResult {
            name: name.to_string(),
            output: serde_json::Value::Null,
            error: Some(detail),
            provider: Some("mcp".into()),
        };
    }

    // Success: surface content faithfully (never silently collapse to "{}").
    let output = build_output(&result);
    ToolResult {
        name: name.to_string(),
        output,
        error: None,
        provider: Some("mcp".into()),
    }
}

/// Extract concatenated text from `result.content` text items, if any.
fn extract_text(result: &serde_json::Value) -> Option<String> {
    let content = result.get("content").and_then(|c| c.as_array())?;
    let mut out = String::new();
    for item in content {
        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                out.push_str(text);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Build the tool output value from a successful `result`, preserving both
/// text (parsed as JSON when possible) and non-text content faithfully.
fn build_output(result: &serde_json::Value) -> serde_json::Value {
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        // If the first item is text, behave like before: parse as JSON when
        // possible, otherwise keep the raw text.
        if content
            .first()
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            == Some("text")
        {
            if let Some(text) = content
                .first()
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
            {
                return serde_json::from_str(text)
                    .unwrap_or_else(|_| serde_json::json!({"text": text}));
            }
        }
        // Non-text (or text content without a `text` field): keep the raw
        // content array instead of collapsing to "{}".
        return serde_json::json!({"content": content});
    }
    result.clone()
}

/// Start an MCP server process and return its instance.
async fn start_server(cfg: &McpServerConfig) -> Result<McpServerInstance> {
    let mut child = Command::new(&cfg.command)
        .args(&cfg.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

    // Spawn reader task
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let writer = tokio::io::BufWriter::new(stdin);

    info!("cafe-mcp-client: started server '{}' (pid {})", cfg.name, child.id().unwrap_or(0));

    Ok(McpServerInstance {
        _process: child,
        writer,
        reader: rx,
    })
}

/// Send a JSON-RPC message to an MCP server.
async fn send_jsonrpc(
    writer: &mut tokio::io::BufWriter<tokio::process::ChildStdin>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<()> {
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&msg)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a JSON-RPC response with matching id from the server's output stream.
async fn read_response(
    rx: &mut tokio::sync::mpsc::Receiver<String>,
    expected_id: u64,
) -> Result<serde_json::Value> {
    use tokio::time::{timeout, Duration};
    let deadline = Duration::from_secs(15);

    timeout(deadline, async {
        while let Some(line) = rx.recv().await {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                if val.get("id").and_then(|v| v.as_u64()) == Some(expected_id) {
                    return Ok(val);
                }
            }
        }
        anyhow::bail!("MCP server disconnected")
    })
    .await
    .map_err(|_| anyhow::anyhow!("timeout waiting for MCP response"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_a_call_ids_are_strictly_increasing() {
        let client = McpClient::new();
        let a = client.next_call_id();
        let b = client.next_call_id();
        let c = client.next_call_id();
        assert_eq!(a, 4, "first tool call id should be 4 (1/2/3 reserved)");
        assert_eq!(b, 5, "second tool call id must increment");
        assert_eq!(c, 6, "third tool call id must increment");
        assert!(a < b && b < c, "call ids must be strictly increasing across calls");
    }

    #[test]
    fn bug_b_iserror_is_reported_as_failure() {
        let resp = serde_json::json!({
            "result": {
                "isError": true,
                "content": [{"type": "text", "text": "boom"}]
            }
        });
        let tr = parse_tool_result(&resp, "my_tool");
        assert!(tr.error.is_some(), "isError:true must be reported as a failure");
        assert_eq!(tr.error.unwrap(), "boom");
        assert!(tr.output.is_null());
    }

    #[test]
    fn bug_b_nontext_content_is_not_collapsed() {
        let resp = serde_json::json!({
            "result": {
                "content": [{"type": "image", "data": "abc", "mimeType": "image/png"}]
            }
        });
        let tr = parse_tool_result(&resp, "my_tool");
        assert!(tr.error.is_none());
        let s = serde_json::to_string(&tr.output).unwrap();
        assert_ne!(s, "{}", "non-text content must not be silently collapsed to {{}}");
    }
}
