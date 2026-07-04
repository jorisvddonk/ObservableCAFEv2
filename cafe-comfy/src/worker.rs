use crate::comfyui::ComfyUIClient;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{
    keys, roles, rpc_errors, Chunk, JsonRpcRequest, JsonRpcResponse, ServerMessage,
};
use std::sync::Arc;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(
    socket_path: String,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) {
    cafe_sdk::bus::run_with_reconnect("cafe-comfy", move || {
        let socket = socket_path.clone();
        let comfy = comfy.clone();
        let wf = workflow.clone();
        let inp = input_node.clone();
        async move { subscribe_sessions(&socket, comfy, &wf, &inp).await }
    })
    .await;
}

async fn subscribe_sessions(
    socket_path: &str,
    comfy: Arc<ComfyUIClient>,
    workflow: &serde_json::Value,
    input_node: &str,
) -> anyhow::Result<()> {
    info!("cafe-comfy: starting (subscribe-all mode) on {}", socket_path);

    let client = BusClient::new(socket_path);
    let mut rx = client.subscribe_all().await?;

    let wf = workflow.to_owned();
    let inp = input_node.to_string();

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let client = client.clone();
            let vb = comfy.clone();
            let wf = wf.clone();
            let inp = inp.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session_handler(session_id, client, vb, wf, inp).await {
                    warn!("cafe-comfy: session handler error: {}", e);
                }
            });
        }
    }

    Ok(())
}

async fn run_session_handler(
    session_id: String,
    client: BusClient,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) -> anyhow::Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("comfy.") { continue; }

        info!(
            "cafe-comfy: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_comfy_request(&comfy, &request, &workflow, &input_node, &client, &session_id).await;

        let response = match result {
            Ok(image_chunk_id) => JsonRpcResponse::ok(
                &call_id,
                serde_json::json!({ "chunk_id": image_chunk_id }),
            ),
            Err(e) => {
                error!("cafe-comfy: comfy error for call {}: {}", call_id, e);
                JsonRpcResponse::err(&call_id, rpc_errors::UPSTREAM_ERROR, e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-comfy")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response);
        let _ = client.publish(&session_id, resp_chunk).await;
    }
    Ok(())
}

async fn handle_comfy_request(
    comfy: &ComfyUIClient,
    request: &JsonRpcRequest,
    workflow: &serde_json::Value,
    input_node: &str,
    client: &BusClient,
    session_id: &str,
) -> anyhow::Result<String> {
    let text = request.params["text"].as_str().unwrap_or_default();

    if text.is_empty() {
        anyhow::bail!("comfy.invoke: text param is empty");
    }

    let image_bytes = comfy.generate(workflow, text, input_node).await?;

    let image_chunk = Chunk::new_binary(image_bytes, "image/png", "com.nominal.cafe-comfy")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);

    let image_chunk_id = image_chunk.id.clone();
    let _ = client.publish(session_id, image_chunk).await;

    info!("cafe-comfy: published image chunk {} for session {}", image_chunk_id, session_id);
    Ok(image_chunk_id)
}
