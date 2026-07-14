use cafe_sdk::{keys, Chunk, JsonRpcRequest, JsonRpcResponse, SdkError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The well-known bus session used for HTTP proxy communication.
pub const PROXY_SESSION: &str = "cafe-server.http-proxy";

/// Annotation key for route registration chunks.
pub const HTTP_ROUTE_REGISTER: &str = "cafe.http.route.register";

/// The JSON-RPC method name for HTTP request proxying.
pub const HTTP_REQUEST_HANDLE: &str = "http.request.handle";

/// Route registration published by a bus service to announce an HTTP endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRegistration {
    pub pattern: String,
    pub methods: Vec<String>,
}

/// HTTP request forwarded by cafe-server to the bus service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub query: HashMap<String, String>,
    /// Base64-encoded request body.
    pub body: String,
    pub auth: ProxyAuthInfo,
}

/// Auth info extracted by cafe-server from the bearer token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyAuthInfo {
    pub user_id: String,
    pub token_type: String,
}

/// HTTP response returned by the bus service to cafe-server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    /// Base64-encoded response body.
    pub body: String,
}

/// Publish a route registration on the proxy session.
///
/// The chunk is transient (not persisted). `source.connection` is
/// auto-injected by the bus so cafe-server learns the caller's
/// connection ID for subsequent `direct_to` RPCs.
pub async fn publish_registration(
    client: &cafe_sdk::bus::BusClient,
    reg: &RouteRegistration,
) -> Result<(), SdkError> {
    let chunk = Chunk::new_null("cafe-http-proxy-sdk")
        .with_annotation(HTTP_ROUTE_REGISTER, reg)
        .as_transient();
    client.publish(PROXY_SESSION, chunk).await
}

/// Parse a `RouteRegistration` from a transient chunk.
pub fn parse_registration(chunk: &Chunk) -> Option<RouteRegistration> {
    let value = chunk.annotations.get(HTTP_ROUTE_REGISTER)?;
    serde_json::from_value(value.clone()).ok()
}

/// Publish an RPC request for an incoming HTTP request.
///
/// Uses `publish_direct` so only the target service receives it.
pub async fn publish_request(
    client: &cafe_sdk::bus::BusClient,
    target_connection: &str,
    call_id: &str,
    request: &ProxyRequest,
) -> Result<(), SdkError> {
    let rpc = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: call_id.to_string(),
        method: HTTP_REQUEST_HANDLE.into(),
        params: serde_json::to_value(request).unwrap_or_default(),
    };
    let chunk = Chunk::new_null("cafe-http-proxy-sdk")
        .with_annotation(keys::CAFE_JSONRPC_REQUEST, &rpc)
        .as_transient();
    client.publish_direct(target_connection, PROXY_SESSION, chunk).await
}

/// Parse a `ProxyRequest` from a chunk's JSON-RPC params.
pub fn parse_request(chunk: &Chunk) -> Option<ProxyRequest> {
    let req = chunk.as_rpc_request()?;
    serde_json::from_value(req.params).ok()
}

/// Publish an RPC response for a handled HTTP request.
///
/// Uses `publish_direct` so only cafe-server receives it.
pub async fn publish_response(
    client: &cafe_sdk::bus::BusClient,
    target_connection: &str,
    call_id: &str,
    response: &ProxyResponse,
) -> Result<(), SdkError> {
    let rpc = JsonRpcResponse::ok(call_id, serde_json::to_value(response).unwrap_or_default());
    let chunk = Chunk::new_null("cafe-http-proxy-sdk")
        .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &rpc)
        .as_transient();
    client.publish_direct(target_connection, PROXY_SESSION, chunk).await
}

/// Parse a `ProxyResponse` from a chunk's JSON-RPC response.
pub fn parse_response(chunk: &Chunk) -> Option<ProxyResponse> {
    let resp = chunk.as_rpc_response()?;
    let result = resp.result.as_ref()?;
    serde_json::from_value(result.clone()).ok()
}

/// Encode bytes as base64 for transport over the bus.
pub fn encode_body(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decode base64-encoded body bytes.
pub fn decode_body(encoded: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_registration_roundtrip() {
        let reg = RouteRegistration {
            pattern: "/foo".into(),
            methods: vec!["GET".into()],
        };
        let chunk = Chunk::new_null("test")
            .with_annotation(HTTP_ROUTE_REGISTER, &reg)
            .as_transient();
        let parsed = parse_registration(&chunk).expect("registration should parse");
        assert_eq!(parsed, reg);
    }
}
