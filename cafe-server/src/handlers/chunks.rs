use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use cafe_sdk::{keys, Chunk, ContentType};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

#[derive(Deserialize, Serialize)]
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
        "binary_ref" => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "BinaryRef chunks published via HTTP never receive \
                              write credentials. Use the WebSocket endpoint at \
                              /api/sessions/{session_id}/ws instead."
                })),
            )
                .into_response()
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::proxy::ProxyState;
    use crate::route_registry::RouteRegistryInner;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::{Json, body::Body};
    use cafe_sdk::bus::BusClient;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn make_state(tmpdir: &std::path::Path) -> AppState {
        let db_path = tmpdir.join("test.db");
        let db = Arc::new(
            crate::db::Db::connect(db_path.to_str().unwrap())
                .await
                .unwrap(),
        );
        db.migrate().await.unwrap();
        AppState {
            bus: BusClient::new(tmpdir.join("bus.sock").to_str().unwrap()),
            db,
            proxy_state: Arc::new(ProxyState {
                registry: Arc::new(RouteRegistryInner::new(1024 * 1024, 30, 60)),
                bus: BusClient::new(tmpdir.join("bus.sock").to_str().unwrap()),
                pending: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            }),
        }
    }

    async fn check_binary_ref_rejected(state: AppState) -> (StatusCode, String) {
        let response = send_chunk(
            State(state),
            AuthUser { is_admin: false },
            Path("sess".into()),
            Json(SendChunkRequest {
                content_type: "binary_ref".into(),
                content: None,
                data: None,
                mime_type: Some("audio/wav".into()),
                annotations: None,
            }),
        )
        .await;
        let resp = response.into_response();
        let status = resp.status();
        let parts = resp.into_parts();
        let body_bytes = axum::body::to_bytes(Body::new(parts.1), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        (status, json["error"].as_str().unwrap_or("").to_string())
    }

    #[tokio::test]
    async fn binary_ref_returns_bad_request() {
        let tmpdir = TempDir::new().unwrap();
        let state = make_state(tmpdir.path()).await;
        let (status, _) = check_binary_ref_rejected(state).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn binary_ref_error_mentions_websocket() {
        let tmpdir = TempDir::new().unwrap();
        let state = make_state(tmpdir.path()).await;
        let (_, err) = check_binary_ref_rejected(state).await;
        assert!(err.contains("WebSocket"), "error should mention WebSocket: {}", err);
        assert!(err.contains("write credentials"), "error should mention write credentials: {}", err);
    }

    #[tokio::test]
    async fn text_chunk_not_rejected() {
        let tmpdir = TempDir::new().unwrap();
        let state = make_state(tmpdir.path()).await;

        // Test text and null — should not be rejected (binary might fail with
        // invalid base64, which is a separate pre-existing validation)
        for ct in ["text", "null"] {
            let body = SendChunkRequest {
                content_type: ct.to_string(),
                content: Some("hello".into()),
                data: None,
                mime_type: None,
                annotations: None,
            };
            let response = send_chunk(
                State(state.clone()),
                AuthUser { is_admin: false },
                Path("sess".into()),
                Json(body),
            )
            .await;
            let resp = response.into_response();
            assert_ne!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "content_type={} should not be rejected",
                ct
            );
        }
        // Verify binary_ref is the ONLY content type that gets the WebSocket error
        let body = SendChunkRequest {
            content_type: "binary_ref".into(),
            content: None,
            data: None,
            mime_type: Some("audio/wav".into()),
            annotations: None,
        };
        let response = send_chunk(
            State(state),
            AuthUser { is_admin: false },
            Path("sess".into()),
            Json(body),
        )
        .await;
        let resp = response.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

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
