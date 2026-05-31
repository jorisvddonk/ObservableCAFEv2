use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use cafe_types::{keys, Chunk};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct SendChunkRequest {
    pub content_type: String,
    pub content: Option<String>,
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

pub async fn fetch_web(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let url = match body["url"].as_str() {
        Some(u) => u.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Missing url field" })),
            )
                .into_response()
        }
    };

    let state2 = state.clone();
    let session_id2 = session_id.clone();

    tokio::spawn(async move {
        match fetch_and_publish(&state2, &session_id2, &url).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("web fetch error: {}", e);
                let err_chunk = Chunk::new_null("com.nominal.cafe-server")
                    .with_annotation(keys::WEB_SOURCE_URL, &url)
                    .with_annotation(keys::WEB_ERROR, true)
                    .with_annotation(keys::ERROR_MESSAGE, e.to_string());
                let _ = state2.bus.publish(&session_id2, err_chunk).await;
            }
        }
    });

    StatusCode::ACCEPTED.into_response()
}

async fn fetch_and_publish(state: &AppState, session_id: &str, url: &str) -> anyhow::Result<()> {
    let response = reqwest::get(url).await?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();
    let text = response.text().await?;

    // Strip basic HTML tags
    let stripped = strip_html(&text);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let chunk = Chunk::new_text(stripped, "com.nominal.cafe-server")
        .with_annotation(keys::WEB_SOURCE_URL, url)
        .with_annotation(keys::WEB_CONTENT_TYPE, content_type)
        .with_annotation(keys::WEB_FETCH_TIME, now_ms)
        .with_annotation(
            keys::SECURITY_TRUST_LEVEL,
            serde_json::json!({ "trusted": false, "source": "web" }),
        );

    state.bus.publish(session_id, chunk).await?;
    Ok(())
}

fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
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
        .with_annotation(keys::FLOW_SIGNAL, "delete")
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
