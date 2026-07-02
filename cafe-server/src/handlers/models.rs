use crate::AppState;
use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use cafe_sdk::ContentType;
use serde_json::json;

pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let session_id = "_cafe_llm_registry";

    match state.bus.get_history(session_id).await {
        Ok(chunks) => {
            let models = chunks
                .iter()
                .filter_map(|c| {
                    if c.content_type == ContentType::Null {
                        c.get_annotation::<String>("config.available_models")
                    } else {
                        None
                    }
                })
                .last();

            match models {
                Some(json_str) => {
                    if let Ok(list) = serde_json::from_str::<Vec<String>>(&json_str) {
                        return Json(json!({ "models": list })).into_response();
                    }
                }
                None => {}
            }

            Json(json!({ "models": [] })).into_response()
        }
        Err(_) => Json(json!({ "models": [] })).into_response(),
    }
}
