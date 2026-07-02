use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use cafe_sdk::{keys, roles, Chunk, ServerMessage};
use serde::Deserialize;
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Deserialize)]
pub struct ChatRequest {
    pub content: String,
}

pub async fn chat(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Json(body): Json<ChatRequest>,
) -> impl IntoResponse {
    // 1. Publish user chunk
    let user_chunk = Chunk::new_text(body.content, "com.nominal.cafe-server")
        .with_annotation(keys::CHAT_ROLE, roles::USER);

    if let Err(e) = state.bus.publish(&session_id, user_chunk).await {
        return axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::from(format!("Bus error: {}", e)))
            .unwrap();
    }

    // 2. Subscribe to session output
    let mut bus_rx = match state.bus.subscribe(&session_id).await {
        Ok(r) => r,
        Err(e) => {
            return axum::response::Response::builder()
                .status(500)
                .body(axum::body::Body::from(format!("Subscribe error: {}", e)))
                .unwrap();
        }
    };

    // 3. Bridge bus messages → SSE events via a new channel.
    //    Skip history replay; only forward chunks after HistoryComplete.
    let (sse_tx, sse_rx) = mpsc::channel::<Result<Event, Infallible>>(256);

    tokio::spawn(async move {
        let mut history_done = false;

        while let Some(msg) = bus_rx.recv().await {
            match msg {
                ServerMessage::HistoryComplete { .. } => {
                    history_done = true;
                }
                ServerMessage::Chunk { chunk, .. } if history_done => {
                    let is_complete = chunk
                        .get_annotation::<bool>(keys::CHAT_STREAM_COMPLETE)
                        .unwrap_or(false);

                    if let Ok(data) = serde_json::to_string(&chunk) {
                        let _ = sse_tx.send(Ok(Event::default().data(data))).await;
                    }

                    if is_complete {
                        break;
                    }
                }
                ServerMessage::Error { message, .. } if history_done => {
                    let data = serde_json::json!({ "type": "error", "message": message });
                    if let Ok(s) = serde_json::to_string(&data) {
                        let _ = sse_tx.send(Ok(Event::default().data(s))).await;
                    }
                    break;
                }
                _ => {}
            }
        }
    });

    Sse::new(ReceiverStream::new(sse_rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}
