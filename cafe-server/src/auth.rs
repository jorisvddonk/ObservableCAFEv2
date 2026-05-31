use crate::AppState;
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub token_id: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

fn extract_bearer(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from)
}

#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer(parts).ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing Authorization header" })),
            )
                .into_response()
        })?;

        let row = state
            .db
            .lookup_token(&token)
            .await
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Database error" })),
                )
                    .into_response()
            })?
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Invalid token" })),
                )
                    .into_response()
            })?;

        Ok(AuthUser {
            token_id: row.id,
            is_admin: row.is_admin,
        })
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AdminUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Admin access required" })),
            )
                .into_response());
        }
        Ok(AdminUser(user))
    }
}
