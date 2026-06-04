use crate::voicebox::VoiceboxClient;
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

/// Top-level entry point. Polls for sessions and spawns a handler task per
/// session. Reconnects the polling loop on errors.
pub async fn run_with_reconnect(socket_path: String, voicebox: VoiceboxClient) {
    let voicebox = Arc::new(voicebox);
    loop {
        match poll_sessions(&socket_path, voicebox.clone()).await {
            Ok(()) => {
                info!("cafe-tts: clean shutdown");
                break;
            }
            Err(e) => {
                warn!("cafe-tts: session poller error: {}. Retrying in 2s", e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Poll list_sessions every 2 s (same pattern as cafe-llm). For each new
/// session, spawn a task that subscribes to it and handles TTS RPC requests.
async fn poll_sessions(socket_path: &str, voicebox: Arc<VoiceboxClient>) -> Result<()> {
    info!("cafe-tts: starting session poller on {}", socket_path);

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

                    info!("cafe-tts: discovered session {}", session.session_id);
                    known.insert(session.session_id.clone());

                    let sid = session.session_id.clone();
                    let sp = socket_path.to_string();
                    let vb = voicebox.clone();
                    tokio::spawn(async move {
                        if let Err(e) = run_session_handler(sid.clone(), sp, vb).await {
                            warn!("cafe-tts: session {} handler error: {}", sid, e);
                        }
                    });
                }

                // Prune stale known sessions so they can be re-discovered if recreated
                known.retain(|id| current_ids.contains(id));
            }
            Err(e) => {
                warn!("cafe-tts: list_sessions error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Subscribe to a single session and handle any TTS RPC requests that arrive.
async fn run_session_handler(
    session_id: String,
    socket_path: String,
    voicebox: Arc<VoiceboxClient>,
) -> Result<()> {
    let stream = UnixStream::connect(&socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    // Subscribe to this specific session (gets history replay + live chunks)
    let sub_msg = serde_json::to_string(&ClientMessage::Subscribe {
        session_id: session_id.clone(),
    })? + "\n";
    writer.write_all(sub_msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("cafe-tts: invalid bus message: {}", e);
                continue;
            }
        };

        let chunk = match msg {
            ServerMessage::Chunk { session_id: sid, chunk } if sid == session_id => chunk,
            // Ignore history-complete, errors, chunks for other sessions, etc.
            _ => continue,
        };

        // Only handle null chunks carrying a JSON-RPC request
        let Some(request) = chunk.as_rpc_request() else {
            continue;
        };

        // Only handle tts.* methods
        if !request.method.starts_with("tts.") {
            continue;
        }

        info!(
            "cafe-tts: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_tts_request(&voicebox, &request, &session_id, &mut writer).await;

        // Publish response (success or error)
        let response = match result {
            Ok(audio_chunk_id) => JsonRpcResponse::ok(
                &call_id,
                serde_json::json!({ "chunk_id": audio_chunk_id }),
            ),
            Err(e) => {
                error!("cafe-tts: TTS error for call {}: {}", call_id, e);
                JsonRpcResponse::err(&call_id, rpc_errors::UPSTREAM_ERROR, e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-tts")
            .with_annotation(keys::JSONRPC_RESPONSE, &response);
        publish_chunk(&mut writer, &session_id, resp_chunk).await;
    }

    Ok(())
}

/// Call Voicebox, publish the audio binary chunk, return its ID.
async fn handle_tts_request(
    voicebox: &VoiceboxClient,
    request: &JsonRpcRequest,
    session_id: &str,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<String> {
    let text = request.params["text"].as_str().unwrap_or_default();
    let profile = request.params["profile"].as_str().unwrap_or("default");
    let engine = request.params["engine"].as_str();

    if text.is_empty() {
        anyhow::bail!("tts.invoke: text param is empty");
    }

    let (audio_bytes, mime_type) = voicebox.speak(text, profile, engine).await?;

    // Publish audio BEFORE the RPC response so it's already in history when
    // cafe-agent-runtime receives the response chunk.
    let audio_chunk = Chunk::new_binary(audio_bytes, mime_type, "com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);

    let audio_chunk_id = audio_chunk.id.clone();
    publish_chunk(writer, session_id, audio_chunk).await;

    info!(
        "cafe-tts: published audio chunk {} for session {}",
        audio_chunk_id, session_id
    );

    Ok(audio_chunk_id)
}

async fn publish_chunk(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    session_id: &str,
    chunk: Chunk,
) {
    let msg = ClientMessage::Publish {
        session_id: session_id.to_string(),
        chunk,
    };
    match serde_json::to_string(&msg) {
        Ok(mut json) => {
            json.push('\n');
            if let Err(e) = writer.write_all(json.as_bytes()).await {
                error!("cafe-tts: write error: {}", e);
            }
        }
        Err(e) => error!("cafe-tts: failed to serialize chunk: {}", e),
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
