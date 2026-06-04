use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Standard JSON-RPC 2.0 error codes.
pub mod rpc_errors {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    // Application-level errors (-32000 to -32099)
    pub const SERVICE_UNAVAILABLE: i32 = -32000;
    pub const TIMEOUT: i32 = -32001;
    pub const UPSTREAM_ERROR: i32 = -32002;
}

/// A JSON-RPC 2.0 request, carried in annotation "jsonrpc.request".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always "2.0".
    pub jsonrpc: String,
    /// UUID v4; used for response correlation.
    pub id: String,
    /// Method name, e.g. "tts.invoke", "stt.invoke".
    pub method: String,
    /// Method-specific parameters object.
    pub params: Value,
}

/// A JSON-RPC 2.0 response, carried in annotation "jsonrpc.response".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always "2.0".
    pub jsonrpc: String,
    /// Matches the request id.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new request with a fresh UUID id.
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Uuid::new_v4().to_string(),
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    /// Successful response.
    pub fn ok(id: impl Into<String>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    /// Error response.
    pub fn err(id: impl Into<String>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// True when no error is present.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_request_sets_jsonrpc_version_and_uuid() {
        let req = JsonRpcRequest::new("tts.invoke", serde_json::json!({"text": "hello"}));
        assert_eq!(req.jsonrpc, "2.0");
        assert!(!req.id.is_empty());
        // UUID v4 format: 8-4-4-4-12
        assert_eq!(req.id.len(), 36);
        assert_eq!(req.method, "tts.invoke");
    }

    #[test]
    fn ok_response_roundtrip() {
        let resp = JsonRpcResponse::ok("abc-123", serde_json::json!({"chunk_id": "xyz"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, "abc-123");
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        assert!(resp.is_ok());

        let json = serde_json::to_string(&resp).unwrap();
        let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "abc-123");
        assert!(back.is_ok());
        // error field is skipped when None
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn err_response_roundtrip() {
        let resp = JsonRpcResponse::err("abc-123", rpc_errors::UPSTREAM_ERROR, "Voicebox down");
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, "abc-123");
        assert!(resp.result.is_none());
        assert!(!resp.is_ok());

        let json = serde_json::to_string(&resp).unwrap();
        let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
        let err = back.error.unwrap();
        assert_eq!(err.code, rpc_errors::UPSTREAM_ERROR);
        assert_eq!(err.message, "Voicebox down");
        // result field is skipped when None
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn two_new_requests_have_distinct_ids() {
        let r1 = JsonRpcRequest::new("tts.invoke", serde_json::Value::Null);
        let r2 = JsonRpcRequest::new("tts.invoke", serde_json::Value::Null);
        assert_ne!(r1.id, r2.id);
    }
}
