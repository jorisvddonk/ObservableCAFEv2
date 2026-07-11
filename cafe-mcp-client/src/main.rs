use std::collections::HashMap;
use std::sync::Arc;

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
            let mut call_id_counter = 3u64;

            // Send tools/call with incrementing id
            call_id_counter += 1;
            let call_id = call_id_counter;
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
        };

        match result {
            Ok(resp) => {
                let content = resp["result"]["content"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|c| c["text"].as_str())
                    .unwrap_or("{}");

                let output: serde_json::Value =
                    serde_json::from_str(content).unwrap_or(serde_json::json!({"text": content}));

                let tool_result = ToolResult {
                    name: tool_call.name.clone(),
                    output: output.clone(),
                    error: None,
                    provider: Some("mcp".into()),
                };

                let result_chunk = Chunk::new_null("com.nominal.cafe-mcp-client")
                    .with_annotation(keys::CAFE_TOOL_RESULT, &tool_result)
                    .as_transient()
                    .with_retain(60);
                let _ = client.publish(&session_id, result_chunk).await;

                // Also publish a human-readable text chunk
                let output_text = serde_json::to_string_pretty(&output)
                    .unwrap_or_else(|_| "{}".into());
                let text = format!(
                    "MCP tool completed: {}\n```\n{}\n```",
                    tool_call.name, output_text
                );
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
