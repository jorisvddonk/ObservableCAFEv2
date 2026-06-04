use crate::handlers::{admin, agents, chat, chunks, quickies, sessions, stream};
use crate::AppState;
use axum::{
    routing::{delete, get, patch, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Health
        .route("/health", get(health))
        // Sessions
        .route("/api/sessions", get(sessions::list_sessions))
        .route("/api/sessions", post(sessions::create_session))
        .route("/api/sessions/:id", delete(sessions::delete_session))
        .route("/api/sessions/:id/history", get(sessions::get_history))
        // Agents
        .route("/api/agents", get(agents::list_agents))
        // Messaging
        .route("/api/sessions/:id/chat", post(chat::chat))
        .route("/api/sessions/:id/stream", get(stream::stream_session))
        .route("/api/sessions/:id/chunks", post(chunks::send_chunk))
        .route("/api/sessions/:id/web", post(chunks::fetch_web))
        .route(
            "/api/sessions/:id/chunks/:chunk_id",
            patch(chunks::trust_chunk),
        )
        .route(
            "/api/sessions/:id/chunks/:chunk_id",
            delete(chunks::delete_chunk),
        )
        .route(
            "/api/sessions/:session_id/chunks/:chunk_id/binary",
            get(chunks::get_chunk_binary),
        )
        // Quickies
        .route("/api/quickies", get(quickies::list_quickies))
        .route("/api/quickies", post(quickies::create_quickie))
        .route("/api/quickies/:id", delete(quickies::delete_quickie))
        // Admin
        .route("/api/admin/tokens", get(admin::list_tokens))
        .route("/api/admin/tokens", post(admin::create_token))
        .route("/api/admin/tokens/:id", delete(admin::delete_token))
        .route("/api/admin/agents", get(admin::list_agents))
        .route("/api/admin/agents/reload", post(admin::reload_agents))
        .route("/api/admin/status", get(admin::system_status))
        .layer(cors)
        .with_state(state)
}

async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok", "version": "0.1.0" }))
}
