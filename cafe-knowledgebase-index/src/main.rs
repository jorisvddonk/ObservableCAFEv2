use std::env;

use anyhow::Result;
use cafe_sdk::{keys, Chunk, JsonRpcRequest};

/// CLI for indexing documents into cafe-knowledgebase.
///
/// Usage:
///   cafe-knowledgebase-index <namespace> <file>
///       [--doc-id <id>] [--metadata <json>] [--bus <path>]
///
/// Reads the file, generates an embedding via the embed API, then calls the
/// knowledgebase.index RPC over the bus.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <namespace> <file> [--doc-id <id>] [--metadata <json>] [--bus <path>]", args[0]);
        std::process::exit(1);
    }

    let namespace = &args[1];
    let file_path = &args[2];

    let mut doc_id: Option<String> = None;
    let mut metadata: Option<String> = None;
    let mut bus_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--doc-id" => {
                i += 1;
                doc_id = Some(args[i].clone());
            }
            "--metadata" => {
                i += 1;
                metadata = Some(args[i].clone());
            }
            "--bus" => {
                i += 1;
                bus_path = args[i].clone();
            }
            _ => {
                eprintln!("Unknown flag: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Read file
    let text = std::fs::read_to_string(file_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", file_path, e))?;

    println!("Indexing {} bytes into namespace '{}'", text.len(), namespace);

    // Generate embedding
    let embedding = embed_text(&text).await?;
    println!("Generated embedding ({} dims)", embedding.len());

    // Publish RPC on the bus
    let client = cafe_sdk::bus::BusClient::new(&bus_path);

    // We need a session to publish the RPC on. We'll use a temporary session.
    let session_id = format!("_cafe_knowledgebase_index_{}", uuid::Uuid::new_v4());
    client.create_session(&session_id, "knowledgebase-index", cafe_sdk::SessionConfig::default()).await?;

    let params = serde_json::json!({
        "namespace": namespace,
        "doc_id": doc_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        "text": text,
        "metadata": metadata,
    });

    let rpc = JsonRpcRequest::new("knowledgebase.index", params);
    let chunk = Chunk::new_null("com.nominal.cafe-knowledgebase-index")
        .with_annotation(keys::CAFE_JSONRPC_REQUEST, &rpc)
        .as_transient();

    client.publish(&session_id, chunk).await?;
    println!("Published index RPC (call_id={})", rpc.id);

    // Subscribe for response
    let mut rx = client.subscribe(&session_id).await?;
    use cafe_sdk::ServerMessage;
    while let Some(msg) = rx.recv().await {
        if let ServerMessage::Chunk { chunk, .. } = msg {
            if let Some(resp) = chunk.as_rpc_response() {
                if resp.id == rpc.id {
                    if let Some(result) = resp.result {
                        println!("Indexed: {}", serde_json::to_string_pretty(&result)?);
                    } else if let Some(err) = resp.error {
                        eprintln!("Error: {} (code {})", err.message, err.code);
                    }
                    break;
                }
            }
        }
    }

    // Cleanup
    client.delete_session(&session_id).await?;

    Ok(())
}

/// Embed text by calling the configured embedding API.
async fn embed_text(text: &str) -> Result<Vec<f32>> {
    let url = std::env::var("CAFE_KNOWLEDGEBASE_EMBED_URL")
        .unwrap_or_else(|_| "http://localhost:11434/api/embed".into());
    let model = std::env::var("CAFE_KNOWLEDGEBASE_EMBED_MODEL")
        .unwrap_or_else(|_| "nomic-embed-text".into());

    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "model": model,
        "input": text,
    });

    let resp = client.post(&url).json(&payload).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("embed API error {}: {}", resp.status(), resp.text().await?);
    }

    let body: serde_json::Value = resp.json().await?;

    // Ollama format: { "embeddings": [[f32]] }
    if let Some(embeddings) = body["embeddings"].as_array() {
        if let Some(first) = embeddings.first() {
            return Ok(first.as_array().unwrap_or(&vec![])
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect());
        }
    }

    // OpenAI format: { "data": [{ "embedding": [f32] }] }
    if let Some(data) = body["data"].as_array() {
        if let Some(first) = data.first() {
            if let Some(embedding) = first["embedding"].as_array() {
                return Ok(embedding.iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect());
            }
        }
    }

    // Single embedding: { "embedding": [f32] }
    if let Some(embedding) = body["embedding"].as_array() {
        return Ok(embedding.iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect());
    }

    anyhow::bail!("unable to parse embedding from response: {}", body)
}
