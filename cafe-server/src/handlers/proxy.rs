use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
};
use cafe_http_proxy_sdk::{encode_body, ProxyRequest, PROXY_SESSION};
use cafe_sdk::{keys, Chunk, JsonRpcRequest};
use serde_json::json;
use tokio::sync::oneshot;

use crate::{auth::AuthUser, route_registry::RouteRegistryInner};

/// Shared state for the proxy handler.
#[derive(Clone)]
pub struct ProxyState {
    pub registry: Arc<RouteRegistryInner>,
    pub bus: cafe_sdk::bus::BusClient,
    /// Pending RPC calls: call_id -> oneshot sender.
    pub pending: Arc<tokio::sync::RwLock<HashMap<String, oneshot::Sender<Result<ProxiedResponse, String>>>>>,
}

/// In-memory representation of a proxied HTTP response (decoded body).
pub struct ProxiedResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Handle an incoming request on `/api/ext/*path`.
pub async fn proxy_handler(
    State(ps): State<ProxyState>,
    _auth: AuthUser,
    req: Request,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query_raw = req.uri().query().unwrap_or("").to_string();
    let headers = flatten_headers(req.headers());

    // Read body with size limit
    let body_bytes = match axum::body::to_bytes(req.into_body(), ps.registry.max_body_size()).await {
        Ok(b) => b.to_vec(),
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "Request body too large"}))).into_response();
        }
    };

    // Match route
    let (entry, _path_params) = match ps.registry.match_path(&path, &method).await {
        Some(t) => t,
        None => {
            return (StatusCode::NOT_FOUND, Json(json!({"error": format!("No route for {} {}", method, path)}))).into_response();
        }
    };

    // Build proxy request
    let proxy_req = ProxyRequest {
        method,
        path,
        headers,
        query: parse_query(&query_raw),
        body: encode_body(&body_bytes),
        auth: cafe_http_proxy_sdk::ProxyAuthInfo {
            user_id: if _auth.is_admin { "admin".into() } else { "user".into() },
            token_type: if _auth.is_admin { "admin".into() } else { "user".into() },
        },
    };

    // Oneshot for response
    let call_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel::<Result<ProxiedResponse, String>>();
    {
        let mut pending = ps.pending.write().await;
        pending.insert(call_id.clone(), tx);
    }

    // Publish direct_to RPC
    let rpc = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: call_id.clone(),
        method: cafe_http_proxy_sdk::HTTP_REQUEST_HANDLE.into(),
        params: serde_json::to_value(&proxy_req).unwrap_or_default(),
    };
    let chunk = Chunk::new_null("cafe-server")
        .with_annotation(keys::CAFE_JSONRPC_REQUEST, &rpc)
        .as_transient();

    if let Err(e) = ps.bus.publish_direct(&entry.connection_id, PROXY_SESSION, chunk).await {
        let mut pending = ps.pending.write().await;
        pending.remove(&call_id);
        return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("dispatch failed: {}", e)}))).into_response();
    }

    // Await with timeout
    let timeout = Duration::from_secs(30);
    let result = tokio::time::timeout(timeout, rx).await;

    // Clean up pending
    {
        let mut pending = ps.pending.write().await;
        pending.remove(&call_id);
    }

    match result {
        Ok(Ok(Ok(resp))) => {
            let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);
            let mut resp_headers = HeaderMap::new();
            for (k, v) in &resp.headers {
                if let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_str(v),
                ) {
                    resp_headers.insert(name, value);
                }
            }
            (status, resp_headers, resp.body).into_response()
        }
        Ok(Ok(Err(e))) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response(),
        Ok(Err(_)) => (StatusCode::BAD_GATEWAY, Json(json!({"error": "service disconnected"}))).into_response(),
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, Json(json!({"error": "service timeout"}))).into_response(),
    }
}

fn flatten_headers(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            map.insert(name.to_string(), v.to_string());
        }
    }
    map
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        if let Some(idx) = pair.find('=') {
            let key = url_decode(&pair[..idx]);
            let value = url_decode(&pair[idx + 1..]);
            map.insert(key, value);
        }
    }
    map
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next();
            let h2 = bytes.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(d) = u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16) {
                    out.push(d as char);
                    continue;
                }
            }
        }
        out.push(b as char);
    }
    out
}
