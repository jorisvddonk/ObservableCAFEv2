use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use cafe_sdk::{keys, Chunk, ContentType};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct SendChunkRequest {
    pub content_type: String,
    pub content: Option<String>,
    pub data: Option<String>,
    pub mime_type: Option<String>,
    pub annotations: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
pub struct TrustRequest {
    pub trusted: bool,
}

pub async fn send_chunk(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Json(body): Json<SendChunkRequest>,
) -> impl IntoResponse {
    let mut chunk = match body.content_type.as_str() {
        "text" => Chunk::new_text(
            body.content.unwrap_or_default(),
            "com.nominal.cafe-server",
        ),
        "null" => Chunk::new_null("com.nominal.cafe-server"),
        "binary" => {
            let raw = match body.data.as_ref().and_then(|d| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, d).ok()
            }) {
                Some(b) => b,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "Invalid base64 data" })),
                    )
                        .into_response()
                }
            };
            let mime = body.mime_type.clone().unwrap_or_else(|| "application/octet-stream".into());
            Chunk::new_binary(raw, mime, "com.nominal.cafe-server")
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid content_type" })),
            )
                .into_response()
        }
    };

    if let Some(annotations) = body.annotations {
        for (k, v) in annotations {
            chunk = chunk.with_annotation(k, v);
        }
    }

    match state.bus.publish(&session_id, chunk).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn trust_chunk(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((session_id, chunk_id)): Path<(String, String)>,
    Json(body): Json<TrustRequest>,
) -> impl IntoResponse {
    // Publish a null chunk that updates trust for the given chunk_id
    let trust_chunk = Chunk::new_null("com.nominal.cafe-server")
        .with_annotation(
            keys::SECURITY_TRUST_LEVEL,
            serde_json::json!({
                "trusted": body.trusted,
                "source": "user",
                "chunk_id": chunk_id
            }),
        );

    match state.bus.publish(&session_id, trust_chunk).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn delete_chunk(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((session_id, chunk_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Publish a flow signal to mark the chunk as deleted
    let del_chunk = Chunk::new_null("com.nominal.cafe-server")
        .with_annotation(keys::CAFE_FLOW_SIGNAL, "delete")
        .with_annotation("flow.target_chunk_id", chunk_id);

    match state.bus.publish(&session_id, del_chunk).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/sessions/:session_id/chunks/:chunk_id/binary
///
/// Returns the raw binary data for a single chunk. Suitable for use as an
/// `<audio src>` or `<img src>` URL. Aggressively cacheable by chunk_id since
/// chunks are immutable.
pub async fn get_chunk_binary(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((session_id, chunk_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Fetch session history from the bus and find the chunk by id
    let history = match state.bus.get_history(&session_id).await {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let chunk = match history.into_iter().find(|c| c.id == chunk_id) {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Chunk not found" })),
            )
                .into_response();
        }
    };

    if chunk.content_type != ContentType::Binary {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Chunk is not binary" })),
        )
            .into_response();
    }

    let data = chunk.data.unwrap_or_default();
    let mime = chunk
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".into());
    let len = data.len();

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CACHE_CONTROL,
                "immutable, max-age=31536000".to_string(),
            ),
        ],
        data,
    )
        .into_response()
}
