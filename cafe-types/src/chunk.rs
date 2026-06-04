use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Text,
    Binary,
    Null,
}

/// base64 serde module for Option<Vec<u8>>
pub mod base64_option {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match data {
            None => serializer.serialize_none(),
            Some(bytes) => serializer.serialize_str(&STANDARD.encode(bytes)),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(None),
            Some(s) => STANDARD
                .decode(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub content_type: ContentType,
    pub content: Option<String>,
    #[serde(with = "base64_option")]
    pub data: Option<Vec<u8>>,
    pub mime_type: Option<String>,
    pub producer: String,
    pub annotations: HashMap<String, serde_json::Value>,
    pub timestamp: i64,
}

impl Chunk {
    fn now_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    pub fn new_text(content: impl Into<String>, producer: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content_type: ContentType::Text,
            content: Some(content.into()),
            data: None,
            mime_type: None,
            producer: producer.into(),
            annotations: HashMap::new(),
            timestamp: Self::now_ms(),
        }
    }

    pub fn new_binary(
        data: Vec<u8>,
        mime_type: impl Into<String>,
        producer: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content_type: ContentType::Binary,
            content: None,
            data: Some(data),
            mime_type: Some(mime_type.into()),
            producer: producer.into(),
            annotations: HashMap::new(),
            timestamp: Self::now_ms(),
        }
    }

    pub fn new_null(producer: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content_type: ContentType::Null,
            content: None,
            data: None,
            mime_type: None,
            producer: producer.into(),
            annotations: HashMap::new(),
            timestamp: Self::now_ms(),
        }
    }

    /// Returns a clone of self with an additional annotation.
    pub fn with_annotation(
        mut self,
        key: impl Into<String>,
        value: impl Serialize,
    ) -> Self {
        let v = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
        self.annotations.insert(key.into(), v);
        self
    }

    /// Convenience: get annotation as a specific type.
    pub fn get_annotation<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.annotations
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Convenience: get chat role.
    pub fn role(&self) -> Option<&str> {
        self.annotations.get("chat.role")?.as_str()
    }

    /// True if this is a runtime config null chunk.
    pub fn is_runtime_config(&self) -> bool {
        self.content_type == ContentType::Null
            && self
                .annotations
                .get("config.type")
                .and_then(|v| v.as_str())
                == Some("runtime")
    }

    /// If this chunk carries a JSON-RPC request, return it.
    pub fn as_rpc_request(&self) -> Option<crate::jsonrpc::JsonRpcRequest> {
        self.get_annotation(crate::annotation::keys::JSONRPC_REQUEST)
    }

    /// If this chunk carries a JSON-RPC response, return it.
    pub fn as_rpc_response(&self) -> Option<crate::jsonrpc::JsonRpcResponse> {
        self.get_annotation(crate::annotation::keys::JSONRPC_RESPONSE)
    }

    /// True if this is a JSON-RPC response matching the given call id.
    pub fn is_rpc_response_for(&self, call_id: &str) -> bool {
        self.as_rpc_response()
            .map(|r| r.id == call_id)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_chunk_roundtrip() {
        let chunk = Chunk::new_text("hello", "com.test");
        let json = serde_json::to_string(&chunk).unwrap();
        let back: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, Some("hello".into()));
        assert_eq!(back.content_type, ContentType::Text);
        assert_eq!(back.id, chunk.id);
    }

    #[test]
    fn binary_chunk_base64_roundtrip() {
        let data = vec![0u8, 1, 2, 3, 255];
        let chunk = Chunk::new_binary(data.clone(), "application/octet-stream", "com.test");
        let json = serde_json::to_string(&chunk).unwrap();
        let back: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(back.data, Some(data));
        assert_eq!(back.content_type, ContentType::Binary);
    }

    #[test]
    fn with_annotation_does_not_mutate_original() {
        let original = Chunk::new_text("hi", "com.test");
        let annotated = original.clone().with_annotation("chat.role", "user");
        assert!(original.annotations.is_empty());
        assert_eq!(
            annotated.annotations.get("chat.role").unwrap().as_str(),
            Some("user")
        );
    }

    #[test]
    fn null_chunk_roundtrip() {
        let chunk = Chunk::new_null("com.test");
        let json = serde_json::to_string(&chunk).unwrap();
        let back: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content_type, ContentType::Null);
        assert!(back.content.is_none());
        assert!(back.data.is_none());
    }

    #[test]
    fn is_runtime_config() {
        let chunk = Chunk::new_null("com.test")
            .with_annotation("config.type", "runtime")
            .with_annotation("config.model", "gemma3:1b");
        assert!(chunk.is_runtime_config());

        let not_config = Chunk::new_null("com.test");
        assert!(!not_config.is_runtime_config());
    }

    #[test]
    fn rpc_request_roundtrip() {
        use crate::jsonrpc::JsonRpcRequest;
        use crate::annotation::keys;

        let req = JsonRpcRequest::new("tts.invoke", serde_json::json!({"text": "hello"}));
        let call_id = req.id.clone();

        let chunk = Chunk::new_null("com.test").with_annotation(keys::JSONRPC_REQUEST, &req);
        let extracted = chunk.as_rpc_request().unwrap();
        assert_eq!(extracted.id, call_id);
        assert_eq!(extracted.method, "tts.invoke");
    }

    #[test]
    fn rpc_response_is_rpc_response_for() {
        use crate::jsonrpc::JsonRpcResponse;
        use crate::annotation::keys;

        let resp = JsonRpcResponse::ok("my-call-id", serde_json::json!({"chunk_id": "xyz"}));
        let chunk = Chunk::new_null("com.test").with_annotation(keys::JSONRPC_RESPONSE, &resp);

        assert!(chunk.is_rpc_response_for("my-call-id"));
        assert!(!chunk.is_rpc_response_for("other-id"));

        // Chunk without response annotation
        let plain = Chunk::new_null("com.test");
        assert!(!plain.is_rpc_response_for("my-call-id"));
    }
}
