use crate::{auth::AdminUser, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub description: Option<String>,
    pub admin: Option<bool>,
}

pub async fn list_tokens(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match state.db.list_tokens().await {
        Ok(tokens) => {
            let out: Vec<_> = tokens
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "description": t.description,
                        "is_admin": t.is_admin,
                        // Don't expose the raw token value in list
                    })
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn create_token(
    State(state): State<AppState>,
    _admin: AdminUser,
    Json(body): Json<CreateTokenRequest>,
) -> impl IntoResponse {
    match state
        .db
        .create_token(body.description.as_deref(), body.admin.unwrap_or(false))
        .await
    {
        Ok(t) => (
            StatusCode::CREATED,
            Json(json!({
                "id": t.id,
                "token": t.token,
                "description": t.description,
                "is_admin": t.is_admin,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn delete_token(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_token(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Token not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn list_agents(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match state.bus.list_sessions().await {
        Ok(sessions) => {
            let agents: Vec<_> = sessions
                .iter()
                .filter(|s| s.is_background)
                .collect();
            Json(agents).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn system_status(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> impl IntoResponse {
    let session_count = state
        .bus
        .list_sessions()
        .await
        .map(|s| s.len())
        .unwrap_or(0);

    Json(json!({
        "uptime_seconds": 0,
        "session_count": session_count,
        "agent_count": 0,
        "bus_connected": true,
        "store_connected": true,
    }))
    .into_response()
}

pub async fn reload_agents(
    _state: State<AppState>,
    _admin: AdminUser,
) -> impl IntoResponse {
    // Signal cafe-agent-runtime via bus (future implementation)
    Json(json!({ "status": "reload requested" })).into_response()
}
