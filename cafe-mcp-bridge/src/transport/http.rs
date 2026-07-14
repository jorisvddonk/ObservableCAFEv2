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
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next();
            let h2 = bytes.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(d) = u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16) {
                    out.push(d);
                    continue;
                }
                // Malformed escape: re-emit the literal `%XX` bytes.
                out.push(b'%');
                out.push(h1);
                out.push(h2);
                continue;
            }
            // Incomplete escape (not enough bytes): re-emit literally, don't drop.
            out.push(b'%');
            if let Some(h1) = h1 {
                out.push(h1);
            }
            if let Some(h2) = h2 {
                out.push(h2);
            }
            continue;
        }
        out.push(b);
    }
    String::from_utf8_lossy(&out).into_owned()
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
    let (_id, stream) = build_sse_response(sessions, raw_query.as_deref()).await;
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Build the SSE stream for a new session: registers the session, then returns
/// a stream that removes the session from the map as soon as the stream ends
/// or the client disconnects (never via a fixed timeout).
async fn build_sse_response(
    sessions: SseSessions,
    raw_query: Option<&str>,
) -> (String, SessionStream) {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

    // Parse tool patterns from query params. Empty = all tools.
    let tool_patterns = {
        let raw = parse_tool_query(raw_query);
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

    let inner = futures_util::stream::once(async { Ok::<_, std::convert::Infallible>(endpoint_event) })
        .chain(ReceiverStream::new(rx).map(|msg| {
            Ok::<_, std::convert::Infallible>(Event::default().event("message").data(msg))
        }));

    let stream = SessionStream::new(
        inner,
        SessionCleanup {
            sessions: sessions.clone(),
            session_id: session_id.clone(),
        },
    );

    (session_id, stream)
}

async fn remove_session(sessions: &SseSessions, id: &str) {
    sessions.write().await.remove(id);
}

/// Guard that removes a session from the map when dropped (client disconnect
/// or stream completion).
struct SessionCleanup {
    sessions: SseSessions,
    session_id: String,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        let sessions = self.sessions.clone();
        let sid = self.session_id.clone();
        tokio::spawn(async move {
            remove_session(&sessions, &sid).await;
        });
    }
}

/// Stream wrapper that fires `SessionCleanup` when the stream ends (poll
/// returns `None`) or when the wrapper itself is dropped.
struct SessionStream {
    inner: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send>,
    >,
    cleanup: Option<SessionCleanup>,
}

impl SessionStream {
    fn new<S>(inner: S, cleanup: SessionCleanup) -> Self
    where
        S: futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
    {
        SessionStream {
            inner: Box::pin(inner),
            cleanup: Some(cleanup),
        }
    }
}

impl futures_util::Stream for SessionStream {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let res = this.inner.as_mut().poll_next(cx);
        if matches!(res, std::task::Poll::Ready(None)) {
            if let Some(cleanup) = this.cleanup.take() {
                drop(cleanup);
            }
        }
        res
    }
}

impl Drop for SessionStream {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            drop(cleanup);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    // ---- Bug A: url_decode ----
    #[test]
    fn test_url_decode_multibyte_utf8() {
        // %E2%82%AC decodes to the Euro sign (3 UTF-8 bytes).
        assert_eq!(url_decode("%E2%82%AC"), "€");
    }

    #[test]
    fn test_url_decode_ascii_space() {
        assert_eq!(url_decode("hello%20world"), "hello world");
    }

    #[test]
    fn test_url_decode_malformed_trailing_percent() {
        // A lone trailing '%' must be re-emitted literally, not dropped.
        assert_eq!(url_decode("a%"), "a%");
    }

    #[test]
    fn test_url_decode_malformed_nonhex() {
        // A '%' followed by non-hex digits must be re-emitted literally.
        assert_eq!(url_decode("%ZZ"), "%ZZ");
    }

    #[test]
    fn test_url_decode_mixed() {
        assert_eq!(url_decode("price%E2%82%AC%20now"), "price€ now");
    }

    // ---- Bug B: SSE session lifecycle ----
    #[tokio::test]
    async fn test_sse_session_removed_on_disconnect() {
        let sessions: SseSessions = Arc::new(RwLock::new(HashMap::new()));

        // Build a session + SSE stream.
        let (_id, stream) = build_sse_response(sessions.clone(), None).await;

        // Confirm the session was registered.
        assert_eq!(sessions.read().await.len(), 1);

        // Simulate client disconnect: drop the SSE stream. This must trigger
        // prompt removal of the session (not a 3600s sleep).
        drop(stream);

        // Allow the spawned cleanup task to run.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            sessions.read().await.len(),
            0,
            "session must be removed on disconnect, not after a 3600s timer"
        );
    }

    #[tokio::test]
    async fn test_sse_session_removed_on_stream_end() {
        let sessions: SseSessions = Arc::new(RwLock::new(HashMap::new()));
        let id = Uuid::new_v4().to_string();

        // Register a session.
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(16);
        sessions
            .write()
            .await
            .insert(id.clone(), SseSession { tx, tool_patterns: None });
        assert_eq!(sessions.read().await.len(), 1);

        // Build a finite inner stream that ends on its own, wrapping it with the
        // cleanup wrapper. When the stream reaches `None`, the session must be
        // removed (not after a 3600s timer).
        let inner = futures_util::stream::iter(vec![Ok::<_, std::convert::Infallible>(
            Event::default().event("message").data("x"),
        )]);
        let mut stream = SessionStream::new(
            inner,
            SessionCleanup {
                sessions: sessions.clone(),
                session_id: id.clone(),
            },
        );

        // Consume the stream to completion.
        let mut count = 0;
        while let Some(_item) = stream.next().await {
            count += 1;
        }
        assert_eq!(count, 1);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            sessions.read().await.len(),
            0,
            "session must be removed when the SSE stream ends"
        );
    }
}
