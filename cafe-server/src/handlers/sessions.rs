use crate::{auth::AuthUser, binary_ref::{BinaryRefQuery, serialize_chunk}, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use cafe_sdk::SessionConfig;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub agent_id: Option<String>,
    pub config: Option<SessionConfig>,
    pub ui_mode: Option<String>,
}

#[derive(Serialize)]
pub struct CreateSessionResponse {
    pub id: String,
    pub agent_id: String,
}

pub async fn list_sessions(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    match state.bus.list_sessions().await {
        Ok(sessions) => Json(sessions).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn create_session(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let session_id = Uuid::new_v4().to_string();
    let agent_id = body.agent_id.unwrap_or_else(|| "default".into());
    let config = body.config.unwrap_or_default();

    match state
        .bus
        .create_session(&session_id, &agent_id, config)
        .await
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(CreateSessionResponse {
                id: session_id,
                agent_id,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn delete_session(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match state.bus.delete_session(&session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn get_history(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Query(query): Query<BinaryRefQuery>,
) -> impl IntoResponse {
    let use_refs = query.enabled();

    match state.bus.get_history(&session_id).await {
        Ok(chunks) => {
            let serialized: Vec<serde_json::Value> = chunks
                .iter()
                .map(|c| serialize_chunk(c, use_refs))
                .collect();
            Json(json!({ "session_id": session_id, "chunks": serialized })).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
