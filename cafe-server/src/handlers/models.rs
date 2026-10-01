use crate::AppState;
use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use cafe_sdk::{keys, ContentType, ModelCatalog};
use serde_json::json;

pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let session_id = "_cafe_llm_registry";

    let chunks = match state.bus.get_history(session_id).await {
        Ok(chunks) => chunks,
        Err(_) => return Json(json!({ "models": [], "backends": [] })).into_response(),
    };

    // The latest registry chunk carries the authoritative catalog. Fall back to
    // the legacy flat `config.available_models` list for older publishers.
    let null_chunks: Vec<_> = chunks
        .iter()
        .filter(|c| c.content_type == ContentType::Null)
        .collect();

    let catalog = null_chunks
        .iter()
        .rev()
        .find_map(|c| c.get_annotation::<String>(keys::CONFIG_MODEL_CATALOG))
        .and_then(|s| serde_json::from_str::<ModelCatalog>(&s).ok())
        .unwrap_or_default();

    let models = if catalog.backends.is_empty() {
        null_chunks
            .iter()
            .rev()
            .find_map(|c| c.get_annotation::<String>("config.available_models"))
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default()
    } else {
        catalog.all_models()
    };

    Json(json!({
        "models": models,
        "default_backend": catalog.default_backend,
        "backends": catalog.backends,
    }))
    .into_response()
}
