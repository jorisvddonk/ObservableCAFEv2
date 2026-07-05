mod tools;
mod transport;

use std::sync::Arc;

use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcRequest, ServerMessage, SessionConfig};
use clap::Parser;
use serde_json::{json, Value};
use tracing::info;
use uuid::Uuid;

/// Shared state available to all transports.
pub struct AppState {
    pub bus_path: String,
    /// All available tools (unfiltered). Per-client filtering is applied in transport.
    pub all_tools: Vec<&'static tools::ToolDef>,
}

#[derive(Parser)]
#[command(name = "cafe-mcp-bridge")]
struct Args {
    /// Cafe bus socket path
    #[arg(long, default_value = "/tmp/cafe-bus.sock")]
    bus: String,

    /// Tool name patterns to expose (repeatable, glob: *, ?)
    #[arg(long = "tool", default_values = &["*"])]
    tools: Vec<String>,

    /// Transport: stdio (default) or http
    #[arg(long, default_value = "stdio")]
    transport: String,

    /// Port for HTTP transport (default 3100)
    #[arg(long, default_value_t = 3100)]
    port: u16,

    /// Enable cafe_meta_* admin tools (list sessions, publish chunks, etc.)
    #[arg(long, default_value_t = false)]
    meta: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let mut all_tools: Vec<&'static tools::ToolDef> = tools::TOOLS.iter().collect();
    if args.meta {
        all_tools.extend(tools::META_TOOLS.iter());
    }
    info!(
        "cafe-mcp-bridge: {} tools available, transport={}",
        all_tools.len(),
        args.transport
    );

    let state = Arc::new(AppState {
        bus_path: args.bus,
        all_tools,
    });

    match args.transport.as_str() {
        "stdio" => transport::stdio::run(state, args.tools).await?,
        "http" => transport::http::run(state, args.port).await?,
        other => anyhow::bail!("unknown transport: {other}"),
    }

    Ok(())
}

/// Dispatch a single MCP JSON-RPC request.
/// `tool_patterns` — optional per-client tool filters (e.g., from SSE ?tool= query).
/// Returns the response value (can be a notification with no response).
pub async fn handle_mcp_request(
    req: &Value,
    state: &AppState,
    tool_patterns: Option<&[String]>,
) -> Option<Value> {
    let method = req["method"].as_str().unwrap_or("");
    let id = &req["id"];
    let params = &req["params"];

    match method {
        "initialize" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": { "name": "cafe-mcp-bridge", "version": "0.1.0" },
                    "capabilities": { "tools": {} }
                }
            });
            Some(resp)
        }
        "notifications/initialized" => None, // notification — no response
        "tools/list" => {
            let available = match tool_patterns {
                Some(patterns) => {
                    let all = &state.all_tools;
                    all.iter().filter(|t| {
                        patterns.iter().any(|p| tools::matches_pattern(t.name, p))
                    }).copied().collect::<Vec<_>>()
                },
                None => state.all_tools.clone(),
            };
            let tool_list: Vec<Value> = available
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema
                    })
                })
                .collect();
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tool_list }
            });
            Some(resp)
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let arguments = &params["arguments"];

            match dispatch_tool(name, arguments, &state.bus_path).await {
                Ok(content) => {
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": content}],
                            "isError": false
                        }
                    }))
                }
                Err(e) => {
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": format!("Error: {e}")}],
                            "isError": true
                        }
                    }))
                }
            }
        }
        _ => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            });
            Some(resp)
        }
    }
}

/// Dispatch a tool call: either inline or via bus RPC.
async fn dispatch_tool(name: &str, arguments: &Value, bus_path: &str) -> Result<String> {
    let args_map = arguments.as_object().cloned().unwrap_or_default();
    match name {
        "web_fetch" => inline_web_fetch(&args_map).await,
        n if n.starts_with("cafe_meta_") => dispatch_meta(n, &args_map, bus_path).await,
        _ => rpc_dispatch(name, &args_map, bus_path).await,
    }
}

/// Dispatch a cafe_meta_* tool (inline bus operations).
async fn dispatch_meta(
    name: &str,
    args: &serde_json::Map<String, Value>,
    bus_path: &str,
) -> Result<String> {
    let client = BusClient::new(bus_path);
    match name {
        "cafe_meta_ping" => meta_ping(&client).await,
        "cafe_meta_list_sessions" => meta_list_sessions(&client).await,
        "cafe_meta_get_history" => meta_get_history(&client, args).await,
        "cafe_meta_publish_chunk" => meta_publish_chunk(&client, args).await,
        "cafe_meta_delete_session" => meta_delete_session(&client, args).await,
        "cafe_meta_list_agents" => meta_list_agents().await,
        "cafe_meta_list_models" => meta_list_models().await,
        _ => anyhow::bail!("unknown meta tool: {name}"),
    }
}

// ── Meta tool implementations ──

async fn meta_ping(client: &BusClient) -> Result<String> {
    client.ping().await?;
    Ok(json!({"status": "ok", "pong": true}).to_string())
}

async fn meta_list_sessions(client: &BusClient) -> Result<String> {
    let sessions = client.list_sessions().await?;
    let sessions_json: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "session_id": s.session_id,
                "agent_id": s.agent_id,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&sessions_json)?)
}

async fn meta_get_history(
    client: &BusClient,
    args: &serde_json::Map<String, Value>,
) -> Result<String> {
    let session_id = args["session_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing session_id"))?;
    let chunks = client.get_history(session_id).await?;
    Ok(serde_json::to_string_pretty(&chunks)?)
}

async fn meta_publish_chunk(
    client: &BusClient,
    args: &serde_json::Map<String, Value>,
) -> Result<String> {
    let session_id = args["session_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing session_id"))?;
    let text = args["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing text"))?;

    let chunk = Chunk::new_text(text, "cafe-mcp-bridge");
    client.publish(session_id, chunk).await?;
    Ok(json!({"published": true}).to_string())
}

async fn meta_delete_session(
    client: &BusClient,
    args: &serde_json::Map<String, Value>,
) -> Result<String> {
    let session_id = args["session_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing session_id"))?;
    client.delete_session(session_id).await?;
    Ok(json!({"deleted": true}).to_string())
}

async fn meta_list_agents() -> Result<String> {
    let agents_dir = std::path::Path::new("agents");
    let mut agents = Vec::new();
    if agents_dir.is_dir() {
        for entry in std::fs::read_dir(agents_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(table) = content.parse::<toml::Table>() {
                        if let Some(name) = table.get("name").and_then(|v| v.as_str()) {
                            agents.push(json!({
                                "name": name,
                                "file": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                            }));
                        }
                    }
                }
            }
        }
    }
    Ok(serde_json::to_string_pretty(&agents)?)
}

async fn meta_list_models() -> Result<String> {
    let server_url = std::env::var("CAFE_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:4000".into());
    let url = format!("{}/api/models", server_url.trim_end_matches('/'));
    let resp = reqwest::get(&url).await?;
    let text = resp.text().await?;
    Ok(text)
}

/// Inline web fetch: GET URL, strip HTML, return text.
async fn inline_web_fetch(args: &serde_json::Map<String, Value>) -> Result<String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing url"))?;
    let resp = reqwest::get(url).await?;
    let body = resp.text().await?;
    Ok(strip_html(&body))
}

fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
}

/// Dispatch a tool via bus RPC.
async fn rpc_dispatch(
    name: &str,
    args: &serde_json::Map<String, Value>,
    bus_path: &str,
) -> Result<String> {
    let tool = tools::TOOLS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;

    let rpc_method = tool
        .rpc_method
        .ok_or_else(|| anyhow::anyhow!("tool {name} has no RPC method"))?;

    let client = BusClient::new(bus_path);

    let session_id = format!("_cafe_mcp_{}", Uuid::new_v4());
    client
        .create_session(&session_id, "cafe-mcp-bridge", SessionConfig::default())
        .await?;

    // Wrap the core logic so session cleanup always runs
    let result = async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let mut rx = client.subscribe(&session_id).await?;
        drain_until_history(&mut rx).await;

        let params = serde_json::to_value(args)?;
        let call_id = Uuid::new_v4().to_string();
        let rpc = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: call_id.clone(),
            method: rpc_method.into(),
            params,
        };
        let chunk = Chunk::new_null("cafe-mcp-bridge")
            .with_annotation(keys::CAFE_JSONRPC_REQUEST, &rpc)
            .as_transient()
            .with_retain(60);
        client.publish(&session_id, chunk).await?;

        wait_for_rpc_response(&mut rx, &call_id).await
    }
    .await;

    let _ = client.delete_session(&session_id).await;
    result
}

async fn drain_until_history(rx: &mut tokio::sync::mpsc::Receiver<ServerMessage>) {
    while let Some(msg) = rx.recv().await {
        if matches!(msg, ServerMessage::HistoryComplete { .. }) {
            break;
        }
    }
}

async fn wait_for_rpc_response(
    rx: &mut tokio::sync::mpsc::Receiver<ServerMessage>,
    call_id: &str,
) -> Result<String> {
    use tokio::time::{timeout, Duration};
    let deadline = Duration::from_secs(60);

    let result = timeout(deadline, async {
        while let Some(msg) = rx.recv().await {
            if let ServerMessage::Chunk { chunk, .. } = msg {
                if let Some(resp) = chunk.as_rpc_response() {
                    if resp.id == call_id {
                        if let Some(r) = resp.result {
                            return Ok(serde_json::to_string_pretty(&r)?);
                        }
                        if let Some(err) = resp.error {
                            anyhow::bail!("RPC error ({}): {}", err.code, err.message);
                        }
                        return Ok("{}".into());
                    }
                }
            }
        }
        anyhow::bail!("bus disconnected while waiting for response")
    })
    .await;

    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(e),
        Err(_) => anyhow::bail!("timeout waiting for RPC response"),
    }
}
