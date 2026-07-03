use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcRequest, ServerMessage, ToolCall, ToolResult};
use std::time::Duration;
use tracing::{error, info};

/// Execute a tool call by dispatching a JSON-RPC request on the bus.
#[allow(dead_code)]
pub async fn execute(
    call: &ToolCall,
    session_id: &str,
    client: &BusClient,
) -> Result<()> {
    let request = JsonRpcRequest::new(&call.name, call.parameters.clone());
    let call_id = request.id.clone();

    info!(
        "tool_executor: dispatching {} call_id={} session={}",
        call.name, call_id, session_id
    );

    client.get_history(session_id).await?;

    let req_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
        .with_annotation(keys::JSONRPC_REQUEST, &request)
        .as_transient()
        .with_retain(60);
    client.publish(session_id, req_chunk).await?;

    let mut rx = client.subscribe(session_id).await?;

    let rpc_timeout = Duration::from_secs(30);
    let result = tokio::time::timeout(rpc_timeout, async {
        loop {
            match rx.recv().await {
                Some(ServerMessage::Chunk { chunk, .. }) => {
                    if chunk.is_rpc_response_for(&call_id) {
                        return chunk
                            .as_rpc_response()
                            .ok_or_else(|| anyhow::anyhow!("failed to deserialize RPC response"));
                    }
                }
                Some(_) => continue,
                None => anyhow::bail!("bus disconnected while waiting for RPC response"),
            }
        }
    })
    .await;

    match result {
        Ok(Ok(response)) => {
            if response.is_ok() {
                info!("tool_executor: {} succeeded call_id={}", call.name, call_id);
                let output = response.result.unwrap_or(serde_json::Value::Null);
                let tool_result = ToolResult {
                    name: call.name.clone(),
                    output,
                    error: None,
                };
                let result_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                    .with_annotation(keys::TOOL_RESULT, &tool_result)
                    .as_transient()
                    .with_retain(60);
                client.publish(session_id, result_chunk).await?;
            } else {
                let err = response.error.unwrap();
                error!(
                    "tool_executor: {} error call_id={}: [{}] {}",
                    call.name, call_id, err.code, err.message
                );
                let tool_result = ToolResult {
                    name: call.name.clone(),
                    output: serde_json::Value::Null,
                    error: Some(err.message),
                };
                let result_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                    .with_annotation(keys::TOOL_RESULT, &tool_result)
                    .as_transient()
                    .with_retain(60);
                client.publish(session_id, result_chunk).await?;
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
            let tool_result = ToolResult {
                name: call.name.clone(),
                output: serde_json::Value::Null,
                error: Some(format!("RPC timeout after {}s", rpc_timeout.as_secs())),
            };
            let result_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                .with_annotation(keys::TOOL_RESULT, &tool_result)
                .as_transient();
            client.publish(session_id, result_chunk).await?;
        }
    }

    Ok(())
}
