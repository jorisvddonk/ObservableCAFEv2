//! cafe-rss — RSS/Atom fetch evaluator.
//!
//! A thin bus service exposing `rss-fetch.invoke`: fetch a feed URL, parse
//! it, and return the items in the RPC result. It deliberately does not
//! publish or summarize — the calling agent decides how to present the feed
//! (see `agents-js/rss-summarizer.js`, which formats the items and asks the
//! LLM to summarize them).

mod feed;

use anyhow::Result;
use cafe_sdk::{keys, Chunk, EvaluatorSchema, JsonRpcResponse, ServerMessage};
use tracing::{info, warn};

const DEFAULT_LIMIT: usize = 10;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path =
        std::env::var("CAFE_BUS_SOCKET").unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    cafe_sdk::bus::run_with_reconnect("cafe-rss", move || {
        let sp = socket_path.clone();
        async move { subscribe_all(&sp).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(socket_path: &str) -> Result<()> {
    info!("cafe-rss: starting on {socket_path}");
    let client = cafe_sdk::bus::BusClient::unix(socket_path);

    let schema = EvaluatorSchema {
        name: "rss-fetch".into(),
        description: "RSS/Atom feed fetch + parse — returns feed items as RPC result".into(),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "config.rss.url": { "type": "string", "description": "Feed URL" }
            }
        }),
        rpc_params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Feed URL to fetch" },
                "limit": { "type": "integer", "description": "Max items to return (default 10)" }
            },
            "required": ["url"]
        }),
    };
    if let Err(e) = cafe_sdk::schema::announce_schema(&client, schema).await {
        warn!("cafe-rss: failed to announce schema: {e}");
    }

    let mut rx = client.subscribe_all().await?;
    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c).await {
                    warn!("cafe-rss: session error: {e}");
                }
            });
        }
    }
    Ok(())
}

async fn run_session(session_id: String, client: cafe_sdk::bus::BusClient) -> Result<()> {
    let mut rx = client.subscribe(&session_id).await?;
    // Gate dispatch on history replay completion (ADR-123).
    let mut history_complete = false;

    while let Some(msg) = rx.recv().await {
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

        let Some(request) = chunk.as_rpc_request() else {
            continue;
        };
        if request.method != "rss-fetch.invoke" {
            continue;
        }
        let call_id = request.id.clone();
        let url = request.params["url"]
            .as_str()
            .or_else(|| request.params["text"].as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let limit = request
            .params["limit"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT);

        let response = if url.is_empty() {
            JsonRpcResponse::err(
                &call_id,
                cafe_sdk::rpc_errors::INVALID_PARAMS,
                "rss-fetch.invoke: url param is required",
            )
        } else {
            match fetch_feed(&url, limit).await {
                Ok(feed) => {
                    info!(
                        "cafe-rss: fetched {} item(s) from {}",
                        feed.items.len(),
                        url
                    );
                    JsonRpcResponse::ok(&call_id, serde_json::to_value(&feed)?)
                }
                Err(e) => {
                    warn!("cafe-rss: fetch failed for {url}: {e}");
                    JsonRpcResponse::err(
                        &call_id,
                        cafe_sdk::rpc_errors::UPSTREAM_ERROR,
                        e.to_string(),
                    )
                }
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-rss")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;
    }
    Ok(())
}

/// Fetch and parse a feed, truncated to `limit` items.
async fn fetch_feed(url: &str, limit: usize) -> Result<feed::Feed> {
    let body = reqwest::get(url).await?.error_for_status()?.text().await?;
    let mut parsed = feed::parse_feed(&body)?;
    parsed.items.truncate(limit);
    Ok(parsed)
}
