mod tools;


use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcRequest, ServerMessage, SessionConfig};
use clap::Parser;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "cafe-mcp-bridge")]
struct Args {
    /// Cafe bus socket path
    #[arg(long, default_value = "/tmp/cafe-bus.sock")]
    bus: String,

    /// Tool name patterns to expose (repeatable, glob: *, ?)
    #[arg(long = "tool", default_values = &["*"])]
    tools: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let bus_path = args.bus;

    let filtered_tools = tools::filter_tools(&args.tools);
    info!(
        "cafe-mcp-bridge: exposing {}/{} tools",
        filtered_tools.len(),
        tools::TOOLS.len(),
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::stdout();

    while let Ok(Some(line)) = reader.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                warn!("invalid JSON-RPC: {}", e);
                continue;
            }
        };

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
                write_json(&mut writer, &resp).await?;
            }
            "notifications/initialized" => {
                // notification — no response
            }
            "tools/list" => {
                let tool_list: Vec<Value> = filtered_tools
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
                write_json(&mut writer, &resp).await?;
            }
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                let arguments = &params["arguments"];

                let result = match dispatch_tool(name, arguments, &bus_path).await {
                    Ok(content) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": content}],
                            "isError": false
                        }
                    }),
                    Err(e) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": format!("Error: {}", e)}],
                            "isError": true
                        }
                    }),
                };
                write_json(&mut writer, &result).await?;
            }
            _ => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Method not found: {}", method) }
                });
                write_json(&mut writer, &resp).await?;
            }
        }
    }

    Ok(())
}

type JsonMap = serde_json::Map<String, Value>;

/// Dispatch a tool call: either inline or via bus RPC.
async fn dispatch_tool(name: &str, arguments: &Value, bus_path: &str) -> Result<String> {
    let args_map = arguments.as_object().cloned().unwrap_or_default();

    match name {
        "web_fetch" => inline_web_fetch(&args_map).await,
        _ => rpc_dispatch(name, &args_map, bus_path).await,
    }
}

/// Inline web fetch: GET URL, strip HTML, return text.
async fn inline_web_fetch(args: &JsonMap) -> Result<String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing url"))?;

    let resp = reqwest::get(url).await?;
    let body = resp.text().await?;
    let stripped = strip_html(&body);
    Ok(stripped)
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
async fn rpc_dispatch(name: &str, args: &JsonMap, bus_path: &str) -> Result<String> {
    // Find the tool def to get the RPC method name
    let tool = tools::TOOLS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", name))?;

    let rpc_method = tool
        .rpc_method
        .ok_or_else(|| anyhow::anyhow!("tool {} has no RPC method", name))?;

    let client = BusClient::new(bus_path);

    // Create a temporary session
    let session_id = format!("_cafe_mcp_{}", Uuid::new_v4());
    client
        .create_session(&session_id, "cafe-mcp-bridge", SessionConfig::default())
        .await?;

    // Give bus subscribers time to pick up the session
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Subscribe and drain history
    let mut rx = client.subscribe(&session_id).await?;
    drain_until_history(&mut rx).await;

    // Build RPC params from args
    let params = serde_json::to_value(args)?;

    // Publish RPC request
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

    // Wait for RPC response
    let result = wait_for_rpc_response(&mut rx, &call_id).await?;

    // Cleanup
    let _ = client.delete_session(&session_id).await;

    Ok(result)
}

/// Drain subscription messages until HistoryComplete.
async fn drain_until_history(rx: &mut tokio::sync::mpsc::Receiver<ServerMessage>) {
    while let Some(msg) = rx.recv().await {
        if matches!(msg, ServerMessage::HistoryComplete { .. }) {
            break;
        }
    }
}

/// Read from the subscription until the matching RPC response is found.
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

/// Write a JSON-RPC line to stdout.
async fn write_json(writer: &mut tokio::io::Stdout, value: &Value) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
