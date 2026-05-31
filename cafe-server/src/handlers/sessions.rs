use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use cafe_types::SessionConfig;
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
) -> impl IntoResponse {
    // Subscribe and collect history until history_complete
    match state.bus.subscribe(&session_id).await {
        Ok(mut rx) => {
            let mut chunks = Vec::new();
            while let Some(msg) = rx.recv().await {
                match msg {
                    cafe_types::ServerMessage::Chunk { chunk, .. } => {
                        chunks.push(chunk);
                    }
                    cafe_types::ServerMessage::HistoryComplete { .. } => break,
                    cafe_types::ServerMessage::Error { message, code, .. } => {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(json!({ "error": message, "code": code })),
                        )
                            .into_response();
                    }
                    _ => {}
                }
            }
            Json(json!({ "session_id": session_id, "chunks": chunks })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
