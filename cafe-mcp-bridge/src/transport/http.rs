use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Query, RawQuery, State},
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::StreamExt;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;
use uuid::Uuid;

use crate::{handle_mcp_request, AppState};

/// Per-session state stored on the SSE connection.
struct SseSession {
    /// Sender for pushing MCP response events to the SSE stream.
    tx: tokio::sync::mpsc::Sender<String>,
    /// Optional per-client tool filter patterns (from ?tool= query params).
    tool_patterns: Option<Vec<String>>,
}

type SseSessions = Arc<RwLock<HashMap<String, SseSession>>>;

/// Parse `?tool=xxx` or `?tool=xxx&tool=yyy` from the raw query string.
fn parse_tool_query(query: Option<&str>) -> Vec<String> {
    let Some(query) = query else { return vec![] };
    let mut tools = Vec::new();
    for pair in query.split('&') {
        let Some(eq) = pair.find('=') else { continue };
        let key = &pair[..eq];
        let val = &pair[eq + 1..];
        if key.eq_ignore_ascii_case("tool") && !val.is_empty() {
            tools.push(url_decode(val));
        }
    }
    tools
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next();
            let h2 = bytes.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(d) = u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16) {
                    out.push(d as char);
                    continue;
                }
            }
        }
        out.push(b as char);
    }
    out
}

#[derive(serde::Deserialize)]
struct SessionQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

pub async fn run(state: Arc<AppState>, port: u16) -> Result<()> {
    let sessions: SseSessions = Arc::new(RwLock::new(HashMap::new()));

    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state((state, sessions));

    let addr = format!("0.0.0.0:{port}");
    info!("cafe-mcp-bridge: HTTP transport listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// SSE endpoint: client connects here to receive events.
/// Supports `?tool=<pattern>` query params for per-client tool filtering.
async fn sse_handler(
    State((_app_state, sessions)): State<(Arc<AppState>, SseSessions)>,
    RawQuery(raw_query): RawQuery,
) -> impl IntoResponse {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

    // Parse tool patterns from query params. Empty = all tools.
    let tool_patterns = {
        let raw = parse_tool_query(raw_query.as_deref());
        let filtered: Vec<String> = raw.into_iter().filter(|p| p != "*").collect();
        if filtered.is_empty() { None } else { Some(filtered) }
    };

    // Register session
    sessions.write().await.insert(
        session_id.clone(),
        SseSession { tx, tool_patterns },
    );

    // Send endpoint event
    let endpoint_event = Event::default()
        .event("endpoint")
        .data(format!("/message?sessionId={session_id}"));

    let stream = futures_util::stream::once(async { Ok::<_, std::convert::Infallible>(endpoint_event) })
        .chain(ReceiverStream::new(rx).map(|msg| {
            Ok::<_, std::convert::Infallible>(Event::default().event("message").data(msg))
        }));

    // Cleanup on disconnect
    let sid = session_id.clone();
    let sessions_clone = sessions.clone();
    tokio::spawn(async move {
        // Wait a brief moment for the initial messages, then clean up
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        sessions_clone.write().await.remove(&sid);
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// POST endpoint: client sends JSON-RPC messages here.
async fn message_handler(
    State((app_state, sessions)): State<(Arc<AppState>, SseSessions)>,
    Query(query): Query<SessionQuery>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let session_id = query.session_id;

    // Look up the session
    let session_entry = {
        let sessions = sessions.read().await;
        sessions.get(&session_id).map(|s| (s.tx.clone(), s.tool_patterns.clone()))
    };

    let (tx, patterns) = match session_entry {
        Some(entry) => entry,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "session not found"})),
            );
        }
    };

    // Process the MCP request with optional per-client tool patterns
    if let Some(resp) = handle_mcp_request(&body, &app_state, patterns.as_deref()).await {
        let resp_str = serde_json::to_string(&resp).unwrap_or_default();
        let _ = tx.send(resp_str).await;
    }

    (axum::http::StatusCode::ACCEPTED, Json(serde_json::json!({"ok": true})))
}
