use crate::voicebox::VoiceboxClient;
use cafe_sdk::bus::{BusClient, SessionSubscription};
use cafe_sdk::{
    keys, roles, rpc_errors, Chunk, JsonRpcRequest, JsonRpcResponse, ServerMessage,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

struct PendingUpload {
    audio_bytes: Vec<u8>,
    mime_type: String,
}

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

    let client = BusClient::unix(socket_path);
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
    let mut sub = client.subscribe_session(&session_id).await?;
    let pending: Arc<Mutex<HashMap<String, PendingUpload>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let http = reqwest::Client::new();

    while let Some(msg) = sub.rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        if let Some(target_id) = chunk
            .annotations
            .get("cafe.mutates.target_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        {
            if chunk.producer == "com.nominal.cafe-binary-store" {
                let mut pending_lock = pending.lock().await;
                if let Some(upload) = pending_lock.remove(&target_id) {
                    let write_url = chunk
                        .annotations
                        .get(keys::CAFE_BINARY_WRITE_URL)
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let write_token = chunk
                        .annotations
                        .get(keys::CAFE_BINARY_WRITE_TOKEN)
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    drop(pending_lock);

                    if let (Some(url), Some(token)) = (write_url, write_token) {
                        let sid = session_id.clone();
                        let audio = upload.audio_bytes;
                        let mime = upload.mime_type;
                        let http = http.clone();
                        let chunk_id = target_id.clone();
                        tokio::spawn(async move {
                            let upload_url = format!(
                                "{}?token={}&session_id={}",
                                url, token, sid
                            );
                            match http
                                .post(&upload_url)
                                .header("Content-Type", &mime)
                                .body(audio)
                                .send()
                                .await
                            {
                                Ok(resp) if resp.status().is_success() => {
                                    info!(
                                        "cafe-tts: uploaded audio for BinaryRef {} ({} bytes)",
                                        chunk_id, resp.content_length().unwrap_or(0)
                                    );
                                }
                                Ok(resp) => {
                                    warn!(
                                        "cafe-tts: binary-store upload failed for {}: HTTP {}",
                                        chunk_id,
                                        resp.status()
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "cafe-tts: binary-store upload error for {}: {}",
                                        chunk_id, e
                                    );
                                }
                            }
                        });
                    }
                }
                continue;
            }
        }

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("tts.") { continue; }

        info!(
            "cafe-tts: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result =
            handle_tts_request(&voicebox, &request, &mut sub, &session_id, &pending).await;

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
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = sub.publish(resp_chunk).await;
    }

    Ok(())
}

async fn handle_tts_request(
    voicebox: &VoiceboxClient,
    request: &JsonRpcRequest,
    sub: &mut SessionSubscription,
    session_id: &str,
    pending: &Arc<Mutex<HashMap<String, PendingUpload>>>,
) -> anyhow::Result<String> {
    let text = request.params["text"].as_str().unwrap_or_default();
    let profile = request.params["profile"].as_str().unwrap_or("default");
    let engine = request.params["engine"].as_str();

    if text.is_empty() {
        anyhow::bail!("tts.invoke: text param is empty");
    }

    let (audio_bytes, mime_type) = voicebox.synthesize(text, profile, engine).await?;

    let chunk = Chunk::new_binary_ref(&mime_type, "com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CAFE_BINARY_BYTE_SIZE, audio_bytes.len() as u64);

    let chunk_id = chunk.id.clone();
    let byte_size = audio_bytes.len();

    sub.publish(chunk).await?;

    info!(
        "cafe-tts: published BinaryRef chunk {} for session {} ({} bytes)",
        chunk_id, session_id, byte_size
    );

    pending.lock().await.insert(
        chunk_id.clone(),
        PendingUpload {
            audio_bytes,
            mime_type,
        },
    );

    Ok(chunk_id)
}
