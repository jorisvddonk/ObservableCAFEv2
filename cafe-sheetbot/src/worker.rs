use crate::sheetbot::SheetbotClient;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{
    keys, rpc_errors, Chunk, JsonRpcRequest, JsonRpcResponse, ServerMessage,
};
use std::sync::Arc;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(socket_path: String, sheetbot: Arc<SheetbotClient>) {
    cafe_sdk::bus::run_with_reconnect("cafe-sheetbot", move || {
        let socket = socket_path.clone();
        let sb = sheetbot.clone();
        async move { subscribe_sessions(&socket, sb).await }
    })
    .await;
}

async fn subscribe_sessions(
    socket_path: &str,
    sheetbot: Arc<SheetbotClient>,
) -> anyhow::Result<()> {
    info!("cafe-sheetbot: starting (subscribe-all mode) on {}", socket_path);

    let client = BusClient::new(socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let client = client.clone();
            let sb = sheetbot.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session_handler(session_id, client, sb).await {
                    warn!("cafe-sheetbot: session handler error: {}", e);
                }
            });
        }
    }

    Ok(())
}

async fn run_session_handler(
    session_id: String,
    client: BusClient,
    sheetbot: Arc<SheetbotClient>,
) -> anyhow::Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("sheetbot.") { continue; }

        info!(
            "cafe-sheetbot: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_sheetbot_request(&sheetbot, &request, &client, &session_id).await;

        let response = match result {
            Ok(json_chunk_id) => JsonRpcResponse::ok(
                &call_id,
                serde_json::json!({ "chunk_id": json_chunk_id }),
            ),
            Err(e) => {
                error!("cafe-sheetbot: sheetbot error for call {}: {}", call_id, e);
                JsonRpcResponse::err(&call_id, rpc_errors::UPSTREAM_ERROR, e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-sheetbot")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response);
        let _ = client.publish(&session_id, resp_chunk).await;
    }

    Ok(())
}

async fn handle_sheetbot_request(
    sheetbot: &SheetbotClient,
    request: &JsonRpcRequest,
    client: &BusClient,
    session_id: &str,
) -> anyhow::Result<String> {
    let method = request
        .method
        .strip_prefix("sheetbot.")
        .unwrap_or(&request.method)
        .to_string();

    let result = sheetbot.dispatch(&method, &request.params).await?;

    let json_str = serde_json::to_string(&result)
        .unwrap_or_else(|_| "{}".to_string());
    let json_chunk = Chunk::new_text(json_str, "com.nominal.cafe-sheetbot")
        .with_annotation("mime_type", "application/json");

    let json_chunk_id = json_chunk.id.clone();
    let _ = client.publish(session_id, json_chunk).await;

    info!(
        "cafe-sheetbot: published json chunk {} for session {}",
        json_chunk_id, session_id
    );

    Ok(json_chunk_id)
}
