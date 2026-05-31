use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct CreateQuickieRequest {
    pub name: String,
    pub description: Option<String>,
    pub emoji: Option<String>,
    pub agent_id: String,
    pub starter_message: Option<String>,
    pub config: Option<serde_json::Value>,
    pub ui_mode: Option<String>,
    pub display_order: Option<i64>,
}

pub async fn list_quickies(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    match state.db.list_quickies().await {
        Ok(q) => Json(q).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn create_quickie(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<CreateQuickieRequest>,
) -> impl IntoResponse {
    let config_json = body
        .config
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());

    match state
        .db
        .create_quickie(
            &body.name,
            body.description.as_deref(),
            body.emoji.as_deref(),
            &body.agent_id,
            body.starter_message.as_deref(),
            config_json.as_deref(),
            body.ui_mode.as_deref().unwrap_or("chat"),
            body.display_order.unwrap_or(0),
        )
        .await
    {
        Ok(q) => (StatusCode::CREATED, Json(q)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn delete_quickie(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_quickie(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Quickie not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
