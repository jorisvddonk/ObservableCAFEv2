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
    pub tools: Vec<&'static tools::ToolDef>,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let filtered_tools = tools::filter_tools(&args.tools);
    info!(
        "cafe-mcp-bridge: exposing {}/{} tools, transport={}",
        filtered_tools.len(),
        tools::TOOLS.len(),
        args.transport
    );

    let state = Arc::new(AppState {
        bus_path: args.bus,
        tools: filtered_tools,
    });

    match args.transport.as_str() {
        "stdio" => transport::stdio::run(state).await?,
        "http" => transport::http::run(state, args.port).await?,
        other => anyhow::bail!("unknown transport: {other}"),
    }

    Ok(())
}

/// Dispatch a single MCP JSON-RPC request.
/// Returns the response value (can be a notification with no response).
pub async fn handle_mcp_request(req: &Value, state: &AppState) -> Option<Value> {
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
            let tool_list: Vec<Value> = state
                .tools
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
        _ => rpc_dispatch(name, &args_map, bus_path).await,
    }
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

    let result = wait_for_rpc_response(&mut rx, &call_id).await?;

    let _ = client.delete_session(&session_id).await;
    Ok(result)
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
