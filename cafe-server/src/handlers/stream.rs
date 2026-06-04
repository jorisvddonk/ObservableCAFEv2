use crate::{auth::AuthUser, binary_ref::BinaryRefQuery, sse, AppState};
use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse,
    },
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

/// Persistent SSE stream of all activity on a session (history + live).
/// Pass `?binaryRefs=1` to receive binary-ref objects instead of full base64 payloads.
pub async fn stream_session(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Query(query): Query<BinaryRefQuery>,
) -> impl IntoResponse {
    let use_refs = query.enabled();

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

    let stream = ReceiverStream::new(rx)
        .filter_map(move |msg| sse::message_to_event_with_refs(&msg, use_refs));

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
