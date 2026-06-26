use crate::sheetbot::SheetbotClient;
use anyhow::Result;
use cafe_types::{
    keys, rpc_errors, Chunk, ClientMessage, JsonRpcRequest, JsonRpcResponse, ServerMessage,
    SessionInfo,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(socket_path: String, sheetbot: Arc<SheetbotClient>) {
    loop {
        match poll_sessions(&socket_path, sheetbot.clone()).await {
            Ok(()) => {
                info!("cafe-sheetbot: clean shutdown");
                break;
            }
            Err(e) => {
                warn!("cafe-sheetbot: session poller error: {}. Retrying in 2s", e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn poll_sessions(
    socket_path: &str,
    sheetbot: Arc<SheetbotClient>,
) -> Result<()> {
    info!("cafe-sheetbot: starting session poller on {}", socket_path);

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

                    info!("cafe-sheetbot: discovered session {}", session.session_id);
                    known.insert(session.session_id.clone());

                    let sid = session.session_id.clone();
                    let sp = socket_path.to_string();
                    let sb = sheetbot.clone();
                    tokio::spawn(async move {
                        if let Err(e) = run_session_handler(sid.clone(), sp, sb).await {
                            warn!("cafe-sheetbot: session {} handler error: {}", sid, e);
                        }
                    });
                }

                known.retain(|id| current_ids.contains(id));
            }
            Err(e) => {
                warn!("cafe-sheetbot: list_sessions error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn run_session_handler(
    session_id: String,
    socket_path: String,
    sheetbot: Arc<SheetbotClient>,
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
                warn!("cafe-sheetbot: invalid bus message: {}", e);
                continue;
            }
        };

        let chunk = match msg {
            ServerMessage::Chunk { session_id: sid, chunk } if sid == session_id => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("sheetbot.") { continue; }

        info!(
            "cafe-sheetbot: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_sheetbot_request(&sheetbot, &request, &session_id, &mut writer).await;

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
            .with_annotation(keys::JSONRPC_RESPONSE, &response);
        publish_chunk(&mut writer, &session_id, resp_chunk).await;
    }

    Ok(())
}

async fn handle_sheetbot_request(
    sheetbot: &SheetbotClient,
    request: &JsonRpcRequest,
    session_id: &str,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<String> {
    // Extract the method name after "sheetbot." prefix
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
    publish_chunk(writer, session_id, json_chunk).await;

    info!(
        "cafe-sheetbot: published json chunk {} for session {}",
        json_chunk_id, session_id
    );

    Ok(json_chunk_id)
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
                error!("cafe-sheetbot: write error: {}", e);
            }
        }
        Err(e) => error!("cafe-sheetbot: failed to serialize chunk: {}", e),
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
