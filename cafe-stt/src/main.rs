mod config;
mod transcriber;

use anyhow::Result;
use cafe_sdk::{keys, roles, Chunk, ContentType, JsonRpcResponse, ServerMessage};
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
    // Track binary_ref chunks waiting for upload completion: chunk_id -> (mime_type)
    let mut pending: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        // ── Handle RPC requests ──
        if let Some(request) = chunk.as_rpc_request() {
            if request.method == "stt.invoke" {
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
                continue;
            }
        }

        // ── Auto-transcription: binary_ref with chat.role=user ──
        if chunk.content_type == ContentType::BinaryRef && chunk.role() == Some("user") {
            let ref_id = chunk.id.clone();
            let mime = chunk.mime_type.clone().unwrap_or_else(|| "audio/wav".into());
            info!("cafe-stt: tracking binary_ref {} for auto-transcription", ref_id);
            pending.insert(ref_id, mime);
            continue;
        }

        // ── Auto-transcription: completion event after upload ──
        if chunk.content_type == ContentType::Null {
            let ann = &chunk.annotations;
            if ann.get("cafe.binary.completed").and_then(|v| v.as_bool()) == Some(true) {
                // Find the binary_ref_id from the mutation's target_id
                if let Some(binary_ref_id) = ann.get("cafe.mutates.target_id").and_then(|v| v.as_str()) {
                    if let Some(mime) = pending.remove(binary_ref_id) {
                        info!("cafe-stt: upload completed for {}, transcribing...", binary_ref_id);
                        if let Err(e) = transcribe_binary_ref(
                            &config, &client, &session_id, binary_ref_id, &mime,
                        ).await {
                            warn!("cafe-stt: auto-transcription failed: {}", e);
                        }
                    }
                }
                continue;
            }
        }
    }

    Ok(())
}

/// Transcribe a binary_ref after its upload completed: fetch read credentials from history,
/// download audio from binary-store, transcribe via voicebox, publish assistant chunk.
async fn transcribe_binary_ref(
    config: &Config,
    bus: &cafe_sdk::bus::BusClient,
    session_id: &str,
    binary_ref_id: &str,
    mime_type: &str,
) -> Result<()> {
    let history = bus.get_history(session_id).await?;
    let read_creds = find_read_credentials(&history, binary_ref_id)
        .ok_or_else(|| anyhow::anyhow!("no read credentials found for binary_ref {}", binary_ref_id))?;

    let read_url = read_creds["cafe.binary.read_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing read_url"))?;
    let read_token = read_creds["cafe.binary.read_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing read_token"))?;

    info!("cafe-stt: fetching audio from {}", read_url);
    let resp = reqwest::Client::new()
        .get(read_url)
        .header("Authorization", format!("Bearer {}", read_token))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("binary-store returned {} for {}", resp.status(), read_url);
    }
    let audio = resp.bytes().await?.to_vec();

    let (text, duration) = transcriber::transcribe(
        &config.voicebox_url,
        &audio,
        mime_type,
        None, // language
        None, // model
    )
    .await?;

    let chunk_id = uuid::Uuid::new_v4().to_string();
    info!("cafe-stt: auto-transcribed '{}' ({:.1}s)", text.chars().take(60).collect::<String>(), duration);

    // Publish as assistant text chunk
    let text_chunk = Chunk::new_text(&text, "com.nominal.cafe-stt")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
    let _ = bus.publish(session_id, text_chunk).await;

    Ok(())
}

/// Handle an stt.invoke RPC: transcribe audio and publish result.
///
/// Supports two input modes:
/// - `audio` (base64) — direct audio data in the params
/// - `binary_ref_id` — scan session history for read credentials, download from binary-store
async fn handle_stt(
    config: &Config,
    params: &serde_json::Value,
    bus: &cafe_sdk::bus::BusClient,
    session_id: &str,
) -> Result<(String, String, f64)> {
    let mime_type = params["mime_type"].as_str().unwrap_or("audio/wav");
    let language = params["language"].as_str();
    let model = params["model"].as_str();

    let audio = if let Some(b64) = params["audio"].as_str() {
        base64_decode(b64)?
    } else if let Some(binary_ref_id) = params["binary_ref_id"].as_str() {
        // Scan session history for read credentials matching this binary_ref
        let history = bus.get_history(session_id).await?;
        let read_creds = find_read_credentials(&history, binary_ref_id)
            .ok_or_else(|| anyhow::anyhow!("no read credentials found for binary_ref {}", binary_ref_id))?;

        let read_url = read_creds["cafe.binary.read_url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing read_url in credentials"))?;
        let read_token = read_creds["cafe.binary.read_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing read_token in credentials"))?;

        info!("cafe-stt: fetching audio from {}", read_url);
        let resp = reqwest::Client::new()
            .get(read_url)
            .header("Authorization", format!("Bearer {}", read_token))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("binary-store returned {} for {}", resp.status(), read_url);
        }
        resp.bytes().await?.to_vec()
    } else {
        // No explicit params — scan history for binary_ref chunks with chat.role=user
        info!("cafe-stt: scanning history for binary_ref chunks");
        let history = bus.get_history(session_id).await?;
        let binary_refs: Vec<&Chunk> = history.iter()
            .filter(|c| c.content_type == ContentType::BinaryRef
                && c.role() == Some("user")
                && c.annotations.get("cafe.binary.read_url").is_none())
            .collect();

        if binary_refs.is_empty() {
            anyhow::bail!("no binary_ref chunks found in session history");
        }

        // Process the first unfetched binary_ref (most recent)
        let ref_chunk = binary_refs.last().unwrap();
        let binary_ref_id = &ref_chunk.id;
        let mime = ref_chunk.mime_type.as_deref().unwrap_or("audio/wav");

        let read_creds = find_read_credentials(&history, binary_ref_id)
            .ok_or_else(|| anyhow::anyhow!("no read credentials found for binary_ref {}. Has the audio been uploaded yet?", binary_ref_id))?;
        let read_url = read_creds["cafe.binary.read_url"].as_str().ok_or_else(|| anyhow::anyhow!("missing read_url"))?;
        let read_token = read_creds["cafe.binary.read_token"].as_str().ok_or_else(|| anyhow::anyhow!("missing read_token"))?;

        info!("cafe-stt: fetching audio from {}", read_url);
        let resp = reqwest::Client::new()
            .get(read_url)
            .header("Authorization", format!("Bearer {}", read_token))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("binary-store returned {} for {}", resp.status(), read_url);
        }
        resp.bytes().await?.to_vec()
    };

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

/// Scan session history for a mutation chunk containing binary read credentials
/// matching the given binary_ref chunk_id.
fn find_read_credentials<'a>(
    history: &'a [Chunk],
    binary_ref_id: &str,
) -> Option<&'a std::collections::HashMap<String, serde_json::Value>> {
    for chunk in history {
        if chunk.content_type != cafe_sdk::ContentType::Null {
            continue;
        }
        let ann = &chunk.annotations;
        if ann.get("cafe.mutates.target_id")
            .and_then(|v| v.as_str())
            != Some(binary_ref_id)
        {
            continue;
        }
        if ann.contains_key("cafe.binary.read_url") && ann.contains_key("cafe.binary.read_token") {
            return Some(ann);
        }
    }
    None
}

fn base64_decode(encoded: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
}
