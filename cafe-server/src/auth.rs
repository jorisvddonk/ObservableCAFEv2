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
    pub is_admin: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AdminUser(pub AuthUser);

fn extract_bearer(parts: &Parts) -> Option<String> {
    // 1. Standard Authorization header (used by all non-EventSource clients)
    if let Some(token) = parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from)
    {
        return Some(token);
    }

    // 2. ?token= query parameter — needed because EventSource cannot set headers
    if let Some(query) = parts.uri.query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("token=") {
                // Percent-decode the value
                let decoded = percent_decode(value);
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }

    None
}

/// Minimal percent-decoder for the token query param (handles %xx sequences).
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.bytes().peekable();
    while let Some(b) = chars.next() {
        if b == b'%' {
            // Take next two hex digits
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(decoded) =
                    u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16)
                {
                    out.push(decoded as char);
                    continue;
                }
            }
        }
        out.push(b as char);
    }
    out
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
