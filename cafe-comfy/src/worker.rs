use crate::comfyui::ComfyUIClient;
use anyhow::Result;
use cafe_types::{
    keys, roles, rpc_errors, Chunk, ClientMessage, JsonRpcRequest, JsonRpcResponse, ServerMessage,
    SessionInfo,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(
    socket_path: String,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) {
    loop {
        match poll_sessions(&socket_path, comfy.clone(), workflow.clone(), input_node.clone()).await {
            Ok(()) => {
                info!("cafe-comfy: clean shutdown");
                break;
            }
            Err(e) => {
                warn!("cafe-comfy: session poller error: {}. Retrying in 2s", e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn poll_sessions(
    socket_path: &str,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) -> Result<()> {
    info!("cafe-comfy: starting session poller on {}", socket_path);

    let mut known: HashSet<String> = HashSet::new();

    loop {
        match list_sessions(socket_path).await {
            Ok(sessions) => {
                let current_ids: HashSet<String> =
                    sessions.iter().map(|s| s.session_id.clone()).collect();

                for session in sessions {
                    if known.contains(&session.session_id) {
                        continue;
                    }
                    info!("cafe-comfy: discovered session {}", session.session_id);
                    known.insert(session.session_id.clone());

                    let sid = session.session_id.clone();
                    let sp = socket_path.to_string();
                    let vb = comfy.clone();
                    let wf = workflow.clone();
                    let inp = input_node.clone();
                    tokio::spawn(async move {
                        if let Err(e) = run_session_handler(sid.clone(), sp, vb, wf, inp).await {
                            warn!("cafe-comfy: session {} handler error: {}", sid, e);
                        }
                    });
                }
                known.retain(|id| current_ids.contains(id));
            }
            Err(e) => {
                warn!("cafe-comfy: list_sessions error: {}", e);
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn run_session_handler(
    session_id: String,
    socket_path: String,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) -> Result<()> {
    let stream = UnixStream::connect(&socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    let sub_msg = serde_json::to_string(&ClientMessage::Subscribe {
        session_id: session_id.clone(),
    })? + "\n";
    writer.write_all(sub_msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("cafe-comfy: invalid bus message: {}", e);
                continue;
            }
        };

        let chunk = match msg {
            ServerMessage::Chunk { session_id: sid, chunk } if sid == session_id => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("comfy.") { continue; }

        info!(
            "cafe-comfy: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_comfy_request(&comfy, &request, &workflow, &input_node, &session_id, &mut writer).await;

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
            .with_annotation(keys::JSONRPC_RESPONSE, &response);
        publish_chunk(&mut writer, &session_id, resp_chunk).await;
    }
    Ok(())
}

async fn handle_comfy_request(
    comfy: &ComfyUIClient,
    request: &JsonRpcRequest,
    workflow: &serde_json::Value,
    input_node: &str,
    session_id: &str,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<String> {
    let text = request.params["text"].as_str().unwrap_or_default();

    if text.is_empty() {
        anyhow::bail!("comfy.invoke: text param is empty");
    }

    let image_bytes = comfy.generate(workflow, text, input_node).await?;

    let image_chunk = Chunk::new_binary(image_bytes, "image/png", "com.nominal.cafe-comfy")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);

    let image_chunk_id = image_chunk.id.clone();
    publish_chunk(writer, session_id, image_chunk).await;

    info!("cafe-comfy: published image chunk {} for session {}", image_chunk_id, session_id);
    Ok(image_chunk_id)
}

async fn publish_chunk(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    session_id: &str,
    chunk: Chunk,
) {
    let msg = ClientMessage::Publish { session_id: session_id.to_string(), chunk };
    match serde_json::to_string(&msg) {
        Ok(mut json) => {
            json.push('\n');
            if let Err(e) = writer.write_all(json.as_bytes()).await {
                error!("cafe-comfy: write error: {}", e);
            }
        }
        Err(e) => error!("cafe-comfy: failed to serialize chunk: {}", e),
    }
}

async fn list_sessions(socket_path: &str) -> Result<Vec<SessionInfo>> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();
    let msg = serde_json::to_string(&ClientMessage::ListSessions)? + "\n";
    writer.write_all(msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if let Ok(ServerMessage::SessionsList { sessions }) = serde_json::from_str(&line) {
            return Ok(sessions);
        }
    }
    Ok(vec![])
}
