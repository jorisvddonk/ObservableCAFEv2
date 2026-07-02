use anyhow::Result;
use cafe_types::{keys, Chunk, ClientMessage, JsonRpcRequest, ServerMessage, ToolCall};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{error, info};

/// Execute a tool call by dispatching a JSON-RPC request on the bus.
///
/// The tool's `name` is used directly as the RPC method (e.g. `sheetbot.list_tasks`).
/// Publishes a tool.result chunk on success or error.
pub async fn execute(
    call: &ToolCall,
    session_id: &str,
    socket_path: &str,
    rpc_timeout: Duration,
) -> Result<()> {
    let request = JsonRpcRequest::new(&call.name, call.parameters.clone());
    let call_id = request.id.clone();

    info!(
        "tool_executor: dispatching {} call_id={} session={}",
        call.name, call_id, session_id
    );

    // Connect to the bus
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    // Subscribe to the session
    let sub_msg = serde_json::to_string(&ClientMessage::Subscribe {
        session_id: session_id.to_string(),
    })?
    + "\n";
    writer.write_all(sub_msg.as_bytes()).await?;

    // Drain history replay
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = lines
            .next_line()
            .await?
            .ok_or_else(|| anyhow::anyhow!("bus disconnected"))?;
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if matches!(msg, ServerMessage::HistoryComplete { .. }) {
            break;
        }
    }

    // Publish the RPC request as a null chunk with jsonrpc.request annotation
    let req_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
        .with_annotation(keys::JSONRPC_REQUEST, &request);
    let pub_msg = ClientMessage::Publish {
        session_id: session_id.to_string(),
        chunk: req_chunk,
    };
    let mut pub_json = serde_json::to_string(&pub_msg).unwrap();
    pub_json.push('\n');
    writer.write_all(pub_json.as_bytes()).await?;

    // Await the matching RPC response
    let result = tokio::time::timeout(rpc_timeout, async {
        loop {
            let line = lines
                .next_line()
                .await?
                .ok_or_else(|| anyhow::anyhow!("bus disconnected"))?;
            let msg: ServerMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let ServerMessage::Chunk { chunk, .. } = msg {
                if chunk.is_rpc_response_for(&call_id) {
                    return chunk
                        .as_rpc_response()
                        .ok_or_else(|| anyhow::anyhow!("failed to deserialize RPC response"));
                }
            }
        }
    })
    .await;

    match result {
        Ok(Ok(response)) => {
            if response.is_ok() {
                info!("tool_executor: {} succeeded call_id={}", call.name, call_id);
                // Publish the result as a tool.result chunk
                let output = response.result.unwrap_or(serde_json::Value::Null);
                let tool_result = cafe_types::ToolResult {
                    name: call.name.clone(),
                    output,
                    error: None,
                };
                let result_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                    .with_annotation(keys::TOOL_RESULT, &tool_result);
                let pub_msg = ClientMessage::Publish {
                    session_id: session_id.to_string(),
                    chunk: result_chunk,
                };
                let mut json = serde_json::to_string(&pub_msg).unwrap();
                json.push('\n');
                writer.write_all(json.as_bytes()).await?;
            } else {
                let err = response.error.unwrap();
                error!(
                    "tool_executor: {} error call_id={}: [{}] {}",
                    call.name, call_id, err.code, err.message
                );
                let tool_result = cafe_types::ToolResult {
                    name: call.name.clone(),
                    output: serde_json::Value::Null,
                    error: Some(err.message),
                };
                let result_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                    .with_annotation(keys::TOOL_RESULT, &tool_result);
                let pub_msg = ClientMessage::Publish {
                    session_id: session_id.to_string(),
                    chunk: result_chunk,
                };
                let mut json = serde_json::to_string(&pub_msg).unwrap();
                json.push('\n');
                writer.write_all(json.as_bytes()).await?;
            }
        }
        Ok(Err(e)) => {
            error!("tool_executor: {} deserialization error: {}", call.name, e);
        }
        Err(_) => {
            error!(
                "tool_executor: {} timed out call_id={}",
                call.name, call_id
            );
            let tool_result = cafe_types::ToolResult {
                name: call.name.clone(),
                output: serde_json::Value::Null,
                error: Some(format!("RPC timeout after {}s", rpc_timeout.as_secs())),
            };
            let result_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                .with_annotation(keys::TOOL_RESULT, &tool_result);
            let pub_msg = ClientMessage::Publish {
                session_id: session_id.to_string(),
                chunk: result_chunk,
            };
            let mut json = serde_json::to_string(&pub_msg).unwrap();
            json.push('\n');
            writer.write_all(json.as_bytes()).await?;
        }
    }

    Ok(())
}
