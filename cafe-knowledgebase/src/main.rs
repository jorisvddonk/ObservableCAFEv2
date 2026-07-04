mod embed;
mod index;

use std::sync::Arc;

use anyhow::Result;
use cafe_sdk::{keys, Chunk, JsonRpcResponse, ServerMessage};
use embed::EmbedConfig;
use index::{chunk_text, KnowledgeBase};
use tracing::{info, warn};

struct App {
    kb: KnowledgeBase,
    embed_config: EmbedConfig,
    bus: cafe_sdk::bus::BusClient,
    chunk_size: usize,
    chunk_overlap: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    let embed_config = EmbedConfig::from_env();
    let db_path = std::env::var("CAFE_KNOWLEDGEBASE_DB_PATH")
        .unwrap_or_else(|_| "./knowledgebase.lance".into());
    let chunk_size = std::env::var("CAFE_KNOWLEDGEBASE_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(512);
    let chunk_overlap = std::env::var("CAFE_KNOWLEDGEBASE_CHUNK_OVERLAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);

    info!(
        "cafe-knowledgebase: starting (embed={}, dim={}, chunk={}+{})",
        embed_config.url,
        embed_config.dim,
        chunk_size,
        chunk_overlap,
    );

    let db_path2 = db_path.clone();
    let dim = embed_config.dim;
    cafe_sdk::bus::run_with_reconnect("cafe-knowledgebase", move || {
        let sp = socket_path.clone();
        let app = Arc::new(App {
            kb: KnowledgeBase::new(db_path2.clone(), dim),
            embed_config: embed_config.clone(),
            bus: cafe_sdk::bus::BusClient::new(&sp),
            chunk_size,
            chunk_overlap,
        });
        async move { subscribe_all(app).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(app: Arc<App>) -> Result<()> {
    let mut rx = app.bus.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let a = app.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, a).await {
                    warn!("cafe-knowledgebase: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}

async fn run_session(session_id: String, app: Arc<App>) -> Result<()> {
    let mut rx = app.bus.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        let call_id = request.id.clone();

        let response = match request.method.as_str() {
            "knowledgebase.embed" => handle_embed(&app, &request.params, &call_id).await,
            "knowledgebase.index" => handle_index(&app, &request.params, &call_id).await,
            "knowledgebase.search" => handle_search(&app, &request.params, &call_id).await,
            "knowledgebase.search_with_context" => {
                handle_search_with_context(&app, &request.params, &call_id).await
            }
            "knowledgebase.delete" => handle_delete(&app, &request.params, &call_id).await,
            "knowledgebase.list" => handle_list(&app, &request.params, &call_id).await,
            _ => continue,
        };

        let resp_chunk = match response {
            Ok(r) => Chunk::new_null("com.nominal.cafe-knowledgebase")
                .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &r)
                .as_transient()
                .with_retain(60),
            Err(e) => {
                warn!("cafe-knowledgebase: error: {}", e);
                let err_resp = JsonRpcResponse::err(&call_id, -1, e.to_string());
                Chunk::new_null("com.nominal.cafe-knowledgebase")
                    .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &err_resp)
                    .as_transient()
                    .with_retain(60)
            }
        };
        let _ = app.bus.publish(&session_id, resp_chunk).await;
    }

    Ok(())
}

async fn handle_embed(
    app: &App,
    params: &serde_json::Value,
    call_id: &str,
) -> Result<JsonRpcResponse> {
    let text = params["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing text"))?;
    let embedding = embed::embed_text(&app.embed_config, text).await?;
    Ok(JsonRpcResponse::ok(
        call_id,
        serde_json::json!({ "embedding": embedding }),
    ))
}

async fn handle_index(
    app: &App,
    params: &serde_json::Value,
    call_id: &str,
) -> Result<JsonRpcResponse> {
    let namespace = params["namespace"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing namespace"))?;
    let text = params["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing text"))?;
    let doc_id = params["doc_id"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let metadata = params["metadata"].as_str();
    let chunk_size = params["chunk_size"]
        .as_u64()
        .map(|v| v as usize)
        .unwrap_or(app.chunk_size);
    let chunk_overlap = params["chunk_overlap"]
        .as_u64()
        .map(|v| v as usize)
        .unwrap_or(app.chunk_overlap);

    // 1. Embed and index the full document
    let full_embedding = embed::embed_text(&app.embed_config, text).await?;
    app.kb
        .index(namespace, &doc_id, text, &full_embedding, metadata, &doc_id, -1)
        .await?;

    // 2. Chunk, embed each chunk, index each chunk
    let chunks = chunk_text(text, chunk_size, chunk_overlap);
    let mut chunk_count = 0usize;

    for (i, chunk_text) in chunks.iter().enumerate() {
        let chunk_embedding = embed::embed_text(&app.embed_config, chunk_text).await?;
        let chunk_id = format!("{}--chunk-{}", doc_id, i);
        app.kb
            .index(namespace, &chunk_id, chunk_text, &chunk_embedding, metadata, &doc_id, i as i32)
            .await?;
        chunk_count += 1;
    }

    info!(
        "indexed doc {} in namespace {} ({} chunks + full)",
        doc_id, namespace, chunk_count,
    );
    Ok(JsonRpcResponse::ok(
        call_id,
        serde_json::json!({ "doc_id": doc_id, "chunk_count": chunk_count }),
    ))
}

async fn handle_search(
    app: &App,
    params: &serde_json::Value,
    call_id: &str,
) -> Result<JsonRpcResponse> {
    let namespace = params["namespace"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing namespace"))?;
    let query = params["query"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing query"))?;
    let k = params["k"].as_u64().unwrap_or(5) as usize;

    let embedding = embed::embed_text(&app.embed_config, query).await?;
    let results = app.kb.search(namespace, &embedding, k).await?;

    Ok(JsonRpcResponse::ok(
        call_id,
        serde_json::json!({ "results": results }),
    ))
}

async fn handle_search_with_context(
    app: &App,
    params: &serde_json::Value,
    call_id: &str,
) -> Result<JsonRpcResponse> {
    let namespace = params["namespace"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing namespace"))?;
    let query = params["query"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing query"))?;
    let k = params["k"].as_u64().unwrap_or(5) as usize;
    let context = params["context_chunks"].as_u64().unwrap_or(2) as usize;

    let embedding = embed::embed_text(&app.embed_config, query).await?;
    let results = app
        .kb
        .search_with_context(namespace, &embedding, k, context)
        .await?;

    Ok(JsonRpcResponse::ok(
        call_id,
        serde_json::json!({ "results": results }),
    ))
}

async fn handle_delete(
    app: &App,
    params: &serde_json::Value,
    call_id: &str,
) -> Result<JsonRpcResponse> {
    let namespace = params["namespace"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing namespace"))?;
    let doc_id = params["doc_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing doc_id"))?;
    app.kb.delete(namespace, doc_id).await?;
    Ok(JsonRpcResponse::ok(
        call_id,
        serde_json::json!({ "deleted": true }),
    ))
}

async fn handle_list(
    app: &App,
    params: &serde_json::Value,
    call_id: &str,
) -> Result<JsonRpcResponse> {
    let namespace = params["namespace"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing namespace"))?;
    let docs = app.kb.list(namespace).await?;
    Ok(JsonRpcResponse::ok(
        call_id,
        serde_json::json!({ "documents": docs }),
    ))
}
