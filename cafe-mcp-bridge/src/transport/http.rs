use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Query, State},
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::StreamExt;
use tokio::sync::RwLock;
use serde::Deserialize;

use tokio_stream::wrappers::ReceiverStream;
use tracing::info;
use uuid::Uuid;

use crate::{handle_mcp_request, AppState};

/// Active SSE connections keyed by session ID.
type SseSenders = Arc<RwLock<HashMap<String, tokio::sync::mpsc::Sender<String>>>>;

#[derive(Deserialize)]
struct SessionQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

pub async fn run(state: Arc<AppState>, port: u16) -> Result<()> {
    let senders: SseSenders = Arc::new(RwLock::new(HashMap::new()));

    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state((state, senders));

    let addr = format!("0.0.0.0:{port}");
    info!("cafe-mcp-bridge: HTTP transport listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// SSE endpoint: client connects here to receive events.
async fn sse_handler(
    State((_app_state, senders)): State<(Arc<AppState>, SseSenders)>,
) -> impl IntoResponse {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

    // Register this session
    senders.write().await.insert(session_id.clone(), tx);

    // Send the endpoint event first
    let endpoint_event = Event::default()
        .event("endpoint")
        .data(format!("/message?sessionId={session_id}"));

    // Build a stream that starts with the endpoint event, then forwards messages
    let stream = futures_util::stream::once(async { Ok::<_, std::convert::Infallible>(endpoint_event) })
        .chain(ReceiverStream::new(rx).map(|msg| {
            Ok::<_, std::convert::Infallible>(Event::default().event("message").data(msg))
        }));

    // Clean up on disconnect
    let senders_clone = senders.clone();
    let sid = session_id.clone();
    let cleanup = async move {
        tokio::signal::ctrl_c().await.ok();
    };
    // For simplicity, cleanup when the stream is dropped:
    let _cleanup = tokio::spawn(async move {
        let _ = cleanup.await;
        senders_clone.write().await.remove(&sid);
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// POST endpoint: client sends JSON-RPC messages here.
async fn message_handler(
    State((app_state, senders)): State<(Arc<AppState>, SseSenders)>,
    Query(query): Query<SessionQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let session_id = query.session_id;

    // Look up the SSE sender for this session
    let tx = {
        let senders = senders.read().await;
        senders.get(&session_id).cloned()
    };

    let tx = match tx {
        Some(tx) => tx,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "session not found"})),
            );
        }
    };

    // Process the MCP request
    if let Some(resp) = handle_mcp_request(&body, &app_state).await {
        let resp_str = serde_json::to_string(&resp).unwrap_or_default();
        let _ = tx.send(resp_str).await;
    }

    (axum::http::StatusCode::ACCEPTED, Json(serde_json::json!({"ok": true})))
}
