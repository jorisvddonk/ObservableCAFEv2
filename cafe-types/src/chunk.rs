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
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(data: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let opt: Option<String> = data.as_ref().map(|bytes| STANDARD.encode(bytes));
        opt.serialize(serializer)
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
    #[cfg_attr(feature = "raw-binary-data", serde(with = "crate::codec::raw_bytes_option"))]
    #[cfg_attr(not(feature = "raw-binary-data"), serde(with = "base64_option"))]
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
        let chunk = Chunk::mutation("target-456", "test");
        assert_eq!(chunk.is_mutation(), Some("target-456".into()));
        assert_eq!(chunk.content_type, ContentType::Null);
    }

    // ── Property-based tests (proptest) ──

    use proptest::prelude::*;

    fn any_content_type() -> impl Strategy<Value = ContentType> {
        prop_oneof![
            Just(ContentType::Text),
            Just(ContentType::Binary),
            Just(ContentType::BinaryRef),
            Just(ContentType::Null),
        ]
    }

    fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn annotation_map() -> impl Strategy<Value = std::collections::HashMap<String, serde_json::Value>> {
        prop::collection::hash_map("[a-z._-]{1,15}", arb_json_value(), 0..5)
    }

    fn any_chunk() -> impl Strategy<Value = Chunk> {
        (
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            any_content_type(),
            proptest::option::of(".{0,50}"),
            proptest::option::of(prop::collection::vec(any::<u8>(), 0..50)),
            proptest::option::of("[a-z/._-]{0,30}"),
            "[a-zA-Z0-9._-]{1,30}",
            annotation_map(),
            any::<i64>(),
        )
            .prop_map(
                |(id, content_type, content, data, mime_type, producer, annotations, timestamp)| {
                    Chunk {
                        id,
                        content_type,
                        content,
                        data,
                        mime_type,
                        producer,
                        annotations,
                        timestamp,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn chunk_serde_roundtrip(chunk in any_chunk()) {
            let json = serde_json::to_string(&chunk).unwrap();
            let back: Chunk = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back.id, chunk.id);
            prop_assert_eq!(back.content_type, chunk.content_type);
            prop_assert_eq!(back.content, chunk.content);
            prop_assert_eq!(back.data, chunk.data);
            prop_assert_eq!(back.mime_type, chunk.mime_type);
            prop_assert_eq!(back.producer, chunk.producer);
            prop_assert_eq!(back.timestamp, chunk.timestamp);
            // Compare annotations via JSON value tree (avoids float-precision issues)
            let orig_val = serde_json::to_value(&chunk.annotations).unwrap();
            let back_val = serde_json::to_value(&back.annotations).unwrap();
            prop_assert_eq!(orig_val, back_val);
        }

        #[test]
        fn chunk_serde_preserves_transient(chunk in any_chunk()) {
            let transient = chunk.as_transient();
            let json = serde_json::to_string(&transient).unwrap();
            let back: Chunk = serde_json::from_str(&json).unwrap();
            prop_assert!(back.is_transient());
        }

        #[test]
        fn chunk_serde_preserves_retain(chunk in any_chunk()) {
            let retained = chunk.as_transient().with_retain(42);
            let json = serde_json::to_string(&retained).unwrap();
            let back: Chunk = serde_json::from_str(&json).unwrap();
            prop_assert!(back.is_transient());
            prop_assert_eq!(back.retain_secs(), Some(42));
        }

        #[test]
        fn chunk_serde_preserves_annotations(
            chunk in any_chunk(),
            key in "[a-z._-]{1,10}",
            value in arb_json_value(),
        ) {
            let annotated = chunk.with_annotation(&key, &value);
            let json = serde_json::to_string(&annotated).unwrap();
            let back: Chunk = serde_json::from_str(&json).unwrap();
            // Use JSON value tree comparison for annotation values
            let orig = serde_json::to_value(&annotated.annotations).unwrap();
            let back_ann = serde_json::to_value(&back.annotations).unwrap();
            prop_assert_eq!(orig, back_ann);
        }

        #[test]
        fn chunk_roundtrip_over_mutations(
            chunk in any_chunk(),
            target_id in "[a-f0-9-]{36}",
        ) {
            let mutation = chunk.clone();
            let mutation = Chunk {
                annotations: {
                    let mut ann = mutation.annotations.clone();
                    ann.insert("cafe.mutates.target_id".into(), serde_json::json!(target_id));
                    ann
                },
                ..mutation
            };
            let json = serde_json::to_string(&mutation).unwrap();
            let back: Chunk = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back.is_mutation(), Some(target_id));
        }
    }

    // ── Annotation merging property tests ──

    use proptest::prelude::*;

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner.run(&strategy, |v| { test(v); Ok(()) }).unwrap();
    }

    #[test]
    fn annotation_last_wins() {
        run_proptest(
            ("[a-z._-]{1,10}", arb_json_value(), arb_json_value()),
            |(key, val1, val2): (String, serde_json::Value, serde_json::Value)| {
                let chunk = Chunk::new_null("test")
                    .with_annotation(&key, val1.clone())
                    .with_annotation(&key, val2.clone());
                assert_eq!(chunk.annotations.get(&key), Some(&val2));
            },
        );
    }

    #[test]
    fn annotation_chains_preserve_unique_keys() {
        run_proptest(
            (
                prop::collection::hash_set("[a-z._-]{1,10}", 1..10),
                arb_json_value(),
            ),
            |(keys, val): (std::collections::HashSet<String>, serde_json::Value)| {
                let mut chunk = Chunk::new_null("test");
                let keys_clone: Vec<String> = keys.iter().cloned().collect();
                for key in &keys_clone {
                    chunk = chunk.with_annotation(key, &val);
                }
                assert_eq!(chunk.annotations.len(), keys_clone.len());
                for key in &keys_clone {
                    assert_eq!(chunk.annotations.get(key), Some(&val));
                }
            },
        );
    }

    #[test]
    fn annotation_does_not_affect_other_fields() {
        run_proptest(
            ("[a-z._-]{1,10}", arb_json_value()),
            |(key, val): (String, serde_json::Value)| {
                let content = "hello world";
                let mut chunk = Chunk::new_text(content, "test-producer");
                let original_id = chunk.id.clone();
                let original_ts = chunk.timestamp;
                chunk = chunk.with_annotation(&key, val);
                assert_eq!(chunk.id, original_id);
                assert_eq!(chunk.content, Some(content.into()));
                assert_eq!(chunk.producer, "test-producer");
                assert_eq!(chunk.timestamp, original_ts);
                assert_eq!(chunk.content_type, ContentType::Text);
            },
        );
    }

    #[test]
    fn annotation_multiple_calls_preserve_all() {
        run_proptest(
            prop::collection::vec(
                ("[a-z._-]{1,10}", arb_json_value()),
                0..10,
            ),
            |pairs: Vec<(String, serde_json::Value)>| {
                let mut chunk = Chunk::new_null("test");
                let mut expected: std::collections::HashMap<String, serde_json::Value> =
                    std::collections::HashMap::new();
                for (key, val) in &pairs {
                    chunk = chunk.with_annotation(key, val.clone());
                    expected.insert(key.clone(), val.clone());
                }
                for (key, val) in &expected {
                    assert_eq!(chunk.annotations.get(key), Some(val));
                }
            },
        );
    }

    #[test]
    fn annotation_overwrite_preserves_unrelated_keys() {
        run_proptest(
            (
                (Just("key_a".to_string()), arb_json_value()),
                (Just("key_b".to_string()), arb_json_value()),
                (Just("key_a".to_string()), arb_json_value()),
            ),
            |((k1, v1), (k2, v2), (k3, v3)): (
                (String, serde_json::Value),
                (String, serde_json::Value),
                (String, serde_json::Value),
            )| {
                let chunk = Chunk::new_null("test")
                    .with_annotation(&k1, &v1)
                    .with_annotation(&k2, &v2)
                    .with_annotation(&k3, &v3);
                // key_a should have the last value (v3)
                assert_eq!(chunk.annotations.get(&k1), Some(&v3));
                // key_b should still have v2
                assert_eq!(chunk.annotations.get(&k2), Some(&v2));
            },
        );
    }

    // ── ADR-driven property tests ──

    #[test]
    fn binary_ref_has_no_data_or_content() {
        run_proptest(arb_chunk_strategy(), |chunk: Chunk| {
            let br = Chunk {
                content_type: ContentType::BinaryRef,
                content: None,
                data: None,
                ..chunk
            };
            assert_eq!(br.content_type, ContentType::BinaryRef);
            assert!(br.content.is_none());
            assert!(br.data.is_none());
            // BinaryRef should serialize correctly
            let json = serde_json::to_string(&br).unwrap();
            let back: Chunk = serde_json::from_str(&json).unwrap();
            assert_eq!(back.content_type, ContentType::BinaryRef);
        });
    }

    #[test]
    fn rpc_request_chunk_is_transient() {
        use crate::jsonrpc::JsonRpcRequest;
        run_proptest(
            (".{0,20}", ".{0,20}", arb_json_value()),
            |(method, id, params): (String, String, serde_json::Value)| {
                let req = JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id,
                    method,
                    params,
                };
                let chunk = Chunk::new_null("test")
                    .with_annotation(crate::annotation::keys::CAFE_JSONRPC_REQUEST, &req)
                    .as_transient();
                assert!(chunk.is_transient());
                assert!(chunk.as_rpc_request().is_some());
            },
        );
    }

    #[test]
    fn rpc_response_chunk_is_transient() {
        use crate::jsonrpc::JsonRpcResponse;
        run_proptest(
            (".{0,20}", arb_json_value()),
            |(id, result): (String, serde_json::Value)| {
                let resp = JsonRpcResponse::ok(id, result);
                let chunk = Chunk::new_null("test")
                    .with_annotation(crate::annotation::keys::CAFE_JSONRPC_RESPONSE, &resp)
                    .as_transient();
                assert!(chunk.is_transient());
                assert!(chunk.as_rpc_response().is_some());
            },
        );
    }

    #[test]
    fn mutation_merge_excludes_meta_key() {
        // Simulate the merge logic: mutation annotations are overlaid on target,
        // but mutates.target_id is excluded.
        run_proptest(
            (
                prop::collection::hash_map("[a-z._-]{1,10}", arb_json_value(), 0..5),
                prop::collection::hash_map("[a-z._-]{1,10}", arb_json_value(), 0..5),
            ),
            |(target_ann, mut_ann): (HashMap<String, serde_json::Value>, HashMap<String, serde_json::Value>)| {
                // The merge: overlays mutation annotations on target,
                // excluding mutates.target_id from the result
                let mut merged = target_ann.clone();
                for (k, v) in &mut_ann {
                    if k != "cafe.mutates.target_id" && k != "mutates.target_id" {
                        merged.insert(k.clone(), v.clone());
                    }
                }
                // mutates.target_id should never be in the merged result
                // unless it was in the original target
                let has_meta = merged.contains_key("cafe.mutates.target_id")
                    || merged.contains_key("mutates.target_id");
                // It may still be in the result if the original target had it
                let original_had_meta = target_ann.contains_key("cafe.mutates.target_id")
                    || target_ann.contains_key("mutates.target_id");
                // But the mutation's meta-key must NOT propagate
                let mutation_meta = mut_ann.contains_key("cafe.mutates.target_id")
                    || mut_ann.contains_key("mutates.target_id");
                if mutation_meta && !original_had_meta {
                    assert!(!has_meta, "mutation's meta-key leaked into merge result");
                }
            },
        );
    }

    #[test]
    fn is_runtime_config_property() {
        run_proptest(arb_chunk_strategy(), |mut chunk: Chunk| {
            // Initially not a runtime config
            chunk.content_type = ContentType::Null;
            chunk.annotations.remove("config.type");
            assert!(!chunk.is_runtime_config());

            // With config.type=runtime → true
            chunk.annotations.insert("config.type".into(), serde_json::Value::String("runtime".into()));
            assert!(chunk.is_runtime_config());

            // Non-null type is never runtime config
            chunk.content_type = ContentType::Text;
            assert!(!chunk.is_runtime_config());

            // Wrong config.type value → false
            chunk.content_type = ContentType::Null;
            chunk.annotations.insert("config.type".into(), serde_json::Value::String("other".into()));
            assert!(!chunk.is_runtime_config());
        });
    }

    #[test]
    fn runtime_config_chunk_is_null() {
        run_proptest(arb_chunk_strategy(), |chunk: Chunk| {
            let rc = Chunk {
                content_type: ContentType::Null,
                annotations: [("config.type".into(), serde_json::Value::String("runtime".into()))]
                    .iter().cloned().collect(),
                ..chunk
            };
            assert!(rc.is_runtime_config());
            assert_eq!(rc.content_type, ContentType::Null);
        });
    }

    fn arb_chunk_strategy() -> impl Strategy<Value = Chunk> {
        (
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            prop_oneof![
                Just(ContentType::Text),
                Just(ContentType::Binary),
                Just(ContentType::BinaryRef),
                Just(ContentType::Null),
            ],
            proptest::option::of(".{0,50}"),
            proptest::option::of(prop::collection::vec(any::<u8>(), 0..50)),
            proptest::option::of("[a-z/._-]{0,30}"),
            "[a-zA-Z0-9._-]{1,30}",
            prop::collection::hash_map("[a-z._-]{1,15}", arb_json_value(), 0..5),
            any::<i64>(),
        )
            .prop_map(
                |(id, content_type, content, data, mime_type, producer, annotations, timestamp)| {
                    Chunk {
                        id,
                        content_type,
                        content,
                        data,
                        mime_type,
                        producer,
                        annotations,
                        timestamp,
                    }
                },
            )
    }
}
