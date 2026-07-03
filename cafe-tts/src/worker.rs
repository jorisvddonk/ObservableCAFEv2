use crate::voicebox::VoiceboxClient;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{
    keys, roles, rpc_errors, Chunk, JsonRpcRequest, JsonRpcResponse, ServerMessage,
};
use std::sync::Arc;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(socket_path: String, voicebox: VoiceboxClient) {
    let voicebox = Arc::new(voicebox);
    cafe_sdk::bus::run_with_reconnect("cafe-tts", move || {
        let socket = socket_path.clone();
        let vb = voicebox.clone();
        async move { subscribe_sessions(&socket, vb).await }
    })
    .await;
}

async fn subscribe_sessions(
    socket_path: &str,
    voicebox: Arc<VoiceboxClient>,
) -> anyhow::Result<()> {
    info!("cafe-tts: starting (subscribe-all mode) on {}", socket_path);

    let client = BusClient::new(socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let client = client.clone();
            let vb = voicebox.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session_handler(session_id, client, vb).await {
                    warn!("cafe-tts: session handler error: {}", e);
                }
            });
        }
    }

    Ok(())
}

async fn run_session_handler(
    session_id: String,
    client: BusClient,
    voicebox: Arc<VoiceboxClient>,
) -> anyhow::Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("tts.") { continue; }

        info!(
            "cafe-tts: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_tts_request(&voicebox, &request, &client, &session_id).await;

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
            .with_annotation(keys::JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;
    }

    Ok(())
}

async fn handle_tts_request(
    voicebox: &VoiceboxClient,
    request: &JsonRpcRequest,
    client: &BusClient,
    session_id: &str,
) -> anyhow::Result<String> {
    let text = request.params["text"].as_str().unwrap_or_default();
    let profile = request.params["profile"].as_str().unwrap_or("default");
    let engine = request.params["engine"].as_str();

    if text.is_empty() {
        anyhow::bail!("tts.invoke: text param is empty");
    }

    let (audio_bytes, mime_type) = voicebox.synthesize(text, profile, engine).await?;

    // Always publish BinaryRef — the binary-store is always available.
    // The actual bytes are currently inlined in the response chunk until the
    // direct-to write credentials flow is implemented (TODO: POST to binary-store).
    let binref = Chunk::new_binary_ref(&mime_type, "com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::BINARY_BYTE_SIZE, audio_bytes.len() as u64);

    // Also publish the full audio as Binary (backward compat for consumers
    // that don't support binary-ref fetching yet).
    let audio = Chunk::new_binary(audio_bytes, &mime_type, "com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);

    let chunk_id = binref.id.clone();
    let _ = client.publish(session_id, binref).await;
    let _ = client.publish(session_id, audio).await;

    info!(
        "cafe-tts: published BinaryRef + Binary chunks {} for session {}",
        chunk_id, session_id
    );

    Ok(chunk_id)
}
