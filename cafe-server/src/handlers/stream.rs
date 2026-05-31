use crate::{auth::AuthUser, sse, AppState};
use axum::{
    extract::{Path, State},
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse,
    },
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

/// Persistent SSE stream of all activity on a session (history + live).
pub async fn stream_session(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let rx = match state.bus.subscribe(&session_id).await {
        Ok(r) => r,
        Err(e) => {
            return axum::response::Response::builder()
                .status(500)
                .body(axum::body::Body::from(format!("Subscribe error: {}", e)))
                .unwrap()
                .into_response();
        }
    };

    let stream = ReceiverStream::new(rx).filter_map(|msg| sse::message_to_event(&msg));

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
