mod config;
mod transcriber;

use anyhow::Result;
use cafe_sdk::{keys, roles, Chunk, JsonRpcResponse, ServerMessage};
use config::Config;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let config = Config::from_env();

    info!(
        "cafe-stt: starting (voicebox={})",
        config.voicebox_url
    );

    cafe_sdk::bus::run_with_reconnect("cafe-stt", move || {
        let cfg = config.clone();
        async move { subscribe_all(&cfg).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(config: &Config) -> Result<()> {
    let client = cafe_sdk::bus::BusClient::new(&config.socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            let cfg = config.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c, cfg).await {
                    warn!("cafe-stt: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}

async fn run_session(
    session_id: String,
    client: cafe_sdk::bus::BusClient,
    config: Config,
) -> Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if request.method != "stt.invoke" {
            continue;
        }
        let call_id = request.id.clone();

        info!("cafe-stt: handling stt.invoke call_id={}", call_id);

        let response = match handle_stt(&config, &request.params, &client, &session_id).await {
            Ok((chunk_id, text, duration)) => JsonRpcResponse::ok(
                &call_id,
                serde_json::json!({
                    "chunk_id": chunk_id,
                    "text": text,
                    "duration": duration,
                }),
            ),
            Err(e) => {
                warn!("cafe-stt: transcription error: {}", e);
                JsonRpcResponse::err(&call_id, -1, e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-stt")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;
    }

    Ok(())
}

/// Handle an stt.invoke RPC: decode audio, transcribe, publish result and assistant chunk.
async fn handle_stt(
    config: &Config,
    params: &serde_json::Value,
    bus: &cafe_sdk::bus::BusClient,
    session_id: &str,
) -> Result<(String, String, f64)> {
    let audio_b64 = params["audio"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing audio (base64)"))?;
    let mime_type = params["mime_type"]
        .as_str()
        .unwrap_or("audio/wav");
    let language = params["language"].as_str();
    let model = params["model"].as_str();

    let audio = base64_decode(audio_b64)?;

    let (text, duration) = transcriber::transcribe(
        &config.voicebox_url,
        &audio,
        mime_type,
        language,
        model,
    )
    .await?;

    let chunk_id = uuid::Uuid::new_v4().to_string();

    // Publish the transcription as an assistant text chunk so it
    // appears in conversation without an LLM step.
    let text_chunk = Chunk::new_text(&text, "com.nominal.cafe-stt")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
    let _ = bus.publish(session_id, text_chunk).await;

    Ok((chunk_id, text, duration))
}

fn base64_decode(encoded: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
}
