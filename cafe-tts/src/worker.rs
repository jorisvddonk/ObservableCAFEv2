use crate::speech_server::TtsService;
use cafe_sdk::bus::{BusClient, SessionSubscription};
use cafe_sdk::{
    keys, roles, rpc_errors, Chunk, EvaluatorSchema, JsonRpcRequest, JsonRpcResponse, ServerMessage,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

struct PendingUpload {
    audio_bytes: Vec<u8>,
    mime_type: String,
}

pub async fn run_with_reconnect(socket_path: String, client: TtsService) {
    let client = Arc::new(client);
    cafe_sdk::bus::run_with_reconnect("cafe-tts", move || {
        let socket = socket_path.clone();
        let c = client.clone();
        async move { subscribe_sessions(&socket, c).await }
    })
    .await;
}

async fn subscribe_sessions(
    socket_path: &str,
    client: Arc<TtsService>,
) -> anyhow::Result<()> {
    info!("cafe-tts: starting (subscribe-all mode) on {}", socket_path);

    let bus = BusClient::unix(socket_path);

    let schema = EvaluatorSchema {
        name: "tts".into(),
        description: "Text-to-speech evaluator — synthesizes speech using Voicebox or speech-server".into(),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "config.tts.profile": { "type": "string", "description": "Voice profile name" },
                "config.tts.engine": { "type": "string", "description": "TTS engine name used by the backend (e.g. qwen for voicebox)" },
                "config.tts.backend": { "type": "string", "enum": ["voicebox", "speech-server"], "description": "TTS backend" },
                "config.tts.endpoint": { "type": "string", "description": "TTS service URL override" }
            }
        }),
        rpc_params_schema: serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": { "type": "string", "description": "Text to synthesize" },
                "profile": { "type": "string", "description": "Voice profile name" },
                "engine": { "type": "string", "description": "TTS engine" },
                "backend": { "type": "string", "enum": ["voicebox", "speech-server"], "description": "TTS backend" },
                "endpoint": { "type": "string", "description": "TTS service URL override" }
            }
        }),
    };
    if let Err(e) = cafe_sdk::schema::announce_schema(&bus, schema).await {
        warn!("cafe-tts: failed to announce schema: {}", e);
    }

    let mut rx = bus.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let bus = bus.clone();
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session_handler(session_id, bus, c).await {
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
    tts: Arc<TtsService>,
) -> anyhow::Result<()> {
    let mut sub = client.subscribe_session(&session_id).await?;
    let pending: Arc<Mutex<HashMap<String, PendingUpload>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let http = reqwest::Client::new();

    // Gate dispatch on history replay completion (ADR-123).
    let mut history_complete = false;

    while let Some(msg) = sub.rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                continue;
            }
            _ => continue,
        };

        if !history_complete {
            continue;
        }

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
            handle_tts_request(&tts, &request, &mut sub, &session_id, &pending).await;

        let response = match result {
            Ok(audio_chunk_id) => JsonRpcResponse::ok(
                &call_id,
                serde_json::json!({ "chunk_id": audio_chunk_id }),
            ),
            Err(e) => {
                error!("cafe-tts: TTS error for call {}: {}", call_id, e);
                let err_chunk = Chunk::new_null("com.nominal.cafe-tts")
                    .with_annotation(keys::ERROR_MESSAGE, e.to_string())
                    .with_annotation("error.source", "tts")
                    .with_annotation(keys::TTS_CALL_ID, &call_id);
                let _ = sub.publish(err_chunk).await;
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
    tts: &TtsService,
    request: &JsonRpcRequest,
    sub: &mut SessionSubscription,
    session_id: &str,
    pending: &Arc<Mutex<HashMap<String, PendingUpload>>>,
) -> anyhow::Result<String> {
    let text = request.params["text"].as_str().unwrap_or_default();
    let profile = request.params["profile"].as_str().unwrap_or("default");
    let engine = request.params["engine"].as_str();
    let language = request.params["language"].as_str();
    let backend = request.params["backend"].as_str();
    let endpoint = request.params["endpoint"].as_str();

    if text.is_empty() {
        anyhow::bail!("tts.invoke: text param is empty");
    }

    let gen_chunk = Chunk::new_null("com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation("chat.audio_streaming", true)
        .with_annotation(keys::TTS_CALL_ID, request.id.as_str())
        .as_transient()
        .with_retain(30);
    sub.publish(gen_chunk).await?;

    let (audio_bytes, mime_type) = tts
        .synthesize(backend, endpoint, text, profile, engine, language)
        .await?;

    let chunk = Chunk::new_binary_ref(&mime_type, "com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CAFE_BINARY_BYTE_SIZE, audio_bytes.len() as u64);

    let chunk_id = chunk.id.clone();
    let byte_size = audio_bytes.len();

    sub.publish(chunk).await?;

    let done_chunk = Chunk::new_null("com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation("chat.audio_complete", true)
        .with_annotation(keys::TTS_CALL_ID, request.id.as_str());
    sub.publish(done_chunk).await?;

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
