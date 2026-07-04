use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Text,
    Binary,
    BinaryRef,
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

    /// Create a BinaryRef chunk announcing a binary asset (no inline data).
    pub fn new_binary_ref(mime_type: impl Into<String>, producer: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content_type: ContentType::BinaryRef,
            content: None,
            data: None,
            mime_type: Some(mime_type.into()),
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
        self.get_annotation(crate::annotation::keys::CAFE_JSONRPC_REQUEST)
    }

    /// If this chunk carries a JSON-RPC response, return it.
    pub fn as_rpc_response(&self) -> Option<crate::jsonrpc::JsonRpcResponse> {
        self.get_annotation(crate::annotation::keys::CAFE_JSONRPC_RESPONSE)
    }

    /// True if this is a JSON-RPC response matching the given call id.
    pub fn is_rpc_response_for(&self, call_id: &str) -> bool {
        self.as_rpc_response()
            .map(|r| r.id == call_id)
            .unwrap_or(false)
    }

    /// If this chunk carries a tool.call annotation, return the ToolCall.
    pub fn as_tool_call(&self) -> Option<crate::tools::ToolCall> {
        self.get_annotation(crate::annotation::keys::CAFE_TOOL_CALL)
    }

    /// If this chunk carries a tool.result annotation, return the ToolResult.
    pub fn as_tool_result(&self) -> Option<crate::tools::ToolResult> {
        self.get_annotation(crate::annotation::keys::CAFE_TOOL_RESULT)
    }

    /// Returns true if this chunk is transient — broadcast to live subscribers
    /// but not appended to history or persisted.
    pub fn is_transient(&self) -> bool {
        self.annotations
            .get(crate::annotation::keys::CAFE_TRANSIENT)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Returns a clone of self with `transient: true` set in annotations.
    pub fn as_transient(self) -> Self {
        self.with_annotation(crate::annotation::keys::CAFE_TRANSIENT, true)
    }

    /// Returns the retention period in seconds for this transient chunk.
    /// `None` means the chunk is not retained — it's fire-and-forget.
    pub fn retain_secs(&self) -> Option<u64> {
        if !self.is_transient() {
            return None;
        }
        self.annotations
            .get(crate::annotation::keys::CAFE_TRANSIENT_RETAIN_SECS)
            .and_then(|v| v.as_u64())
    }

    /// Mark this transient chunk as retained for the given number of seconds.
    /// Late subscribers will still receive it within that window.
    pub fn with_retain(self, secs: u64) -> Self {
        self.with_annotation(crate::annotation::keys::CAFE_TRANSIENT_RETAIN_SECS, secs)
    }

    /// Returns true if this chunk announces a binary asset without inline data.
    pub fn is_binary_ref(&self) -> bool {
        matches!(self.content_type, ContentType::BinaryRef)
    }

    /// Returns the target chunk ID if this chunk is a mutation.
    pub fn is_mutation(&self) -> Option<String> {
        self.get_annotation(crate::annotation::keys::CAFE_MUTATES_TARGET_ID)
    }

    /// Create a null chunk that mutates (adds annotations to) the given target.
    pub fn mutation(target_id: &str, producer: &str) -> Self {
        Chunk::new_null(producer)
            .with_annotation(crate::annotation::keys::CAFE_MUTATES_TARGET_ID, target_id)
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

        let chunk = Chunk::new_null("com.test").with_annotation(keys::CAFE_JSONRPC_REQUEST, &req);
        let extracted = chunk.as_rpc_request().unwrap();
        assert_eq!(extracted.id, call_id);
        assert_eq!(extracted.method, "tts.invoke");
    }

    #[test]
    fn rpc_response_is_rpc_response_for() {
        use crate::jsonrpc::JsonRpcResponse;
        use crate::annotation::keys;

        let resp = JsonRpcResponse::ok("my-call-id", serde_json::json!({"chunk_id": "xyz"}));
        let chunk = Chunk::new_null("com.test").with_annotation(keys::CAFE_JSONRPC_RESPONSE, &resp);

        assert!(chunk.is_rpc_response_for("my-call-id"));
        assert!(!chunk.is_rpc_response_for("other-id"));

        // Chunk without response annotation
        let plain = Chunk::new_null("com.test");
        assert!(!plain.is_rpc_response_for("my-call-id"));
    }

    #[test]
    fn is_transient_true_when_annotation_is_true() {
        let chunk = Chunk::new_text("hello", "com.test")
            .with_annotation(crate::annotation::keys::CAFE_TRANSIENT, true);
        assert!(chunk.is_transient());
    }

    #[test]
    fn is_transient_false_when_annotation_absent() {
        let chunk = Chunk::new_text("hello", "com.test");
        assert!(!chunk.is_transient());
    }

    #[test]
    fn is_transient_false_when_annotation_is_false() {
        let chunk = Chunk::new_text("hello", "com.test")
            .with_annotation(crate::annotation::keys::CAFE_TRANSIENT, false);
        assert!(!chunk.is_transient());
    }

    #[test]
    fn as_transient_does_not_mutate_original() {
        let original = Chunk::new_text("hello", "com.test");
        let cloned = original.clone();
        let _transient = cloned.as_transient();
        assert!(!original.is_transient());
    }

    #[test]
    fn as_transient_sets_transient_to_true() {
        let chunk = Chunk::new_text("hello", "com.test").as_transient();
        assert!(chunk.is_transient());
    }

    #[test]
    fn retain_secs_none_for_non_transient() {
        let chunk = Chunk::new_text("hello", "com.test").with_retain(60);
        assert!(chunk.retain_secs().is_none());
    }

    #[test]
    fn retain_secs_for_transient() {
        let chunk = Chunk::new_text("hello", "com.test").as_transient().with_retain(60);
        assert_eq!(chunk.retain_secs(), Some(60));
    }

    #[test]
    fn is_mutation_false_when_not_present() {
        let chunk = Chunk::new_text("hello", "com.test");
        assert!(chunk.is_mutation().is_none());
    }

    #[test]
    fn is_mutation_true_when_present() {
        use crate::annotation::keys;
        let chunk = Chunk::new_null("test").with_annotation(keys::CAFE_MUTATES_TARGET_ID, "target-123");
        assert_eq!(chunk.is_mutation(), Some("target-123".into()));
    }

    #[test]
    fn mutation_constructor_sets_target_id() {
        use crate::annotation::keys;
        let chunk = Chunk::mutation("target-456", "test");
        assert_eq!(chunk.is_mutation(), Some("target-456".into()));
        assert_eq!(chunk.content_type, ContentType::Null);
    }
}
