use std::env;

use anyhow::Result;
use cafe_sdk::{keys, Chunk, JsonRpcRequest, ServerMessage};

/// CLI for indexing documents into cafe-knowledgebase.
///
/// Usage:
///   cafe-knowledgebase-index <namespace> <file>
///       [--doc-id <id>] [--metadata <json>] [--bus <path>]
///       [--chunk-size <N>] [--chunk-overlap <N>]
///
/// Reads the file and calls knowledgebase.index RPC over the bus.
/// The server handles embedding and automatic chunking.
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!(
            "Usage: {} <namespace> <file> [--doc-id <id>] [--metadata <json>] \
             [--bus <path>] [--chunk-size <N>] [--chunk-overlap <N>]",
            args[0]
        );
        std::process::exit(1);
    }

    let namespace = &args[1];
    let file_path = &args[2];

    let mut doc_id: Option<String> = None;
    let mut metadata: Option<String> = None;
    let mut bus_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());
    let mut chunk_size: Option<u64> = None;
    let mut chunk_overlap: Option<u64> = None;

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
            "--chunk-size" => {
                i += 1;
                chunk_size = Some(args[i].parse()?);
            }
            "--chunk-overlap" => {
                i += 1;
                chunk_overlap = Some(args[i].parse()?);
            }
            _ => {
                eprintln!("Unknown flag: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let text = std::fs::read_to_string(file_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", file_path, e))?;

    println!("Indexing {} bytes into namespace '{}'", text.len(), namespace);

    let client = cafe_sdk::bus::BusClient::new(&bus_path);

    let session_id = format!("_cafe_knowledgebase_index_{}", uuid::Uuid::new_v4());
    client.create_session(&session_id, "knowledgebase-index", cafe_sdk::SessionConfig::default()).await?;

    let mut params = serde_json::json!({
        "namespace": namespace,
        "doc_id": doc_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        "text": text,
        "metadata": metadata,
    });
    if let Some(cs) = chunk_size {
        params["chunk_size"] = serde_json::json!(cs);
    }
    if let Some(co) = chunk_overlap {
        params["chunk_overlap"] = serde_json::json!(co);
    }

    let rpc = JsonRpcRequest::new("knowledgebase.index", params);
    let chunk = Chunk::new_null("com.nominal.cafe-knowledgebase-index")
        .with_annotation(keys::CAFE_JSONRPC_REQUEST, &rpc)
        .as_transient();

    client.publish(&session_id, chunk).await?;
    println!("Published index RPC (call_id={})", rpc.id);

    let mut rx = client.subscribe(&session_id).await?;
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

    client.delete_session(&session_id).await?;
    Ok(())
}
