use crate::handlers::{admin, agents, chat, chunks, models, proxy, quickies, sessions, stream, ws_handler};
use crate::AppState;
use axum::routing::{any, delete, get, patch, post};
use axum::Router;
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
        // Models
        .route("/api/models", get(models::list_models))
        // Messaging
        .route("/api/sessions/:id/chat", post(chat::chat))
        .route("/api/sessions/:id/stream", get(stream::stream_session))
        .route("/api/sessions/:id/chunks", post(chunks::send_chunk))
        .route(
            "/api/sessions/:id/chunks/:chunk_id",
            patch(chunks::trust_chunk),
        )
        .route(
            "/api/sessions/:id/chunks/:chunk_id",
            delete(chunks::delete_chunk),
        )
        .route("/api/sessions/:id/ws", get(ws_handler::ws_session))
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
        // Dynamic HTTP proxy routes (registered by bus services)
        .route("/api/ext/*path", any(proxy::proxy_handler))
        .layer(cors)
        .with_state(state)
}

async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok", "version": "0.1.0" }))
}
