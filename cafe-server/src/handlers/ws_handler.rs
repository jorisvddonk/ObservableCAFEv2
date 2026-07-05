use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
};
use cafe_sdk::{Chunk, ServerMessage};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tracing::{info, warn};

/// Deserialized action from a WebSocket client.
#[derive(Deserialize)]
struct WsAction {
    op: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    chunk: Option<Chunk>,
}

/// WebSocket session endpoint.
///
/// `GET /api/sessions/:id/ws?token=<auth>`
///
/// Once upgraded, the client receives session events as JSON messages:
///   {"event":"chunk","chunk":{...}}
///   {"event":"history_complete","count":0}
///
/// And can send actions back:
///   {"op":"publish","chunk":{"content_type":"text","content":"hello"}}
///   {"op":"subscribe","session_id":"<new>"}
///
/// Published chunks are tagged with the server's bus connection ID,
/// so direct_to replies (e.g. binary-store write credentials) arrive
/// here and are forwarded to the WebSocket client.
pub async fn ws_session(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, session_id))
}

async fn handle_ws(mut socket: WebSocket, state: AppState, initial_session: String) {
    let mut current_session = initial_session.clone();

    // Subscribe to the initial session
    let mut bus_rx = match state.bus.subscribe(&current_session).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!("ws_handler: subscribe error: {}", e);
            return;
        }
    };

    loop {
        tokio::select! {
            // ── Incoming from bus → forward to WebSocket ──
            bus_msg = bus_rx.recv() => {
                let payload = match bus_msg {
                    Some(ServerMessage::Chunk { chunk, .. }) => {
                        serde_json::to_string(&serde_json::json!({
                            "event": "chunk",
                            "chunk": chunk,
                        })).unwrap_or_default()
                    }
                    Some(ServerMessage::HistoryComplete { count, .. }) => {
                        serde_json::to_string(&serde_json::json!({
                            "event": "history_complete",
                            "count": count,
                        })).unwrap_or_default()
                    }
                    Some(ServerMessage::Error { message, code, .. }) => {
                        serde_json::to_string(&serde_json::json!({
                            "event": "error",
                            "message": message,
                            "code": code,
                        })).unwrap_or_default()
                    }
                    Some(_) => continue,
                    None => break, // bus disconnected
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }

            // ── Incoming from WebSocket → handle action ──
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        let action: WsAction = match serde_json::from_str(&text) {
                            Ok(a) => a,
                            Err(e) => {
                                warn!("ws_handler: invalid action: {}", e);
                                continue;
                            }
                        };

                        match action.op.as_str() {
                            "publish" => {
                                if let Some(chunk) = action.chunk {
                                    if let Err(e) = state.bus.publish(&current_session, chunk).await {
                                        warn!("ws_handler: publish error: {}", e);
                                    }
                                }
                            }
                            "subscribe" => {
                                if let Some(new_sid) = action.session_id {
                                    info!("ws_handler: switching to session {}", new_sid);
                                    current_session = new_sid.clone();
                                    match state.bus.subscribe(&new_sid).await {
                                        Ok(new_rx) => bus_rx = new_rx,
                                        Err(e) => {
                                            warn!("ws_handler: subscribe error: {}", e);
                                            break;
                                        }
                                    }
                                }
                            }
                            _ => {
                                warn!("ws_handler: unknown op: {}", action.op);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    info!("ws_handler: client disconnected from {}", initial_session);
}
