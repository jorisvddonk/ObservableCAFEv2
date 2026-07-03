use cafe_sdk::{keys, Chunk, ContentType};
use serde::Deserialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Query extractor
// ---------------------------------------------------------------------------

/// Query parameters that opt in to binary-ref substitution.
/// Present as `?binaryRefs=1` in the URL.
#[derive(Debug, Default, Deserialize)]
pub struct BinaryRefQuery {
    #[serde(rename = "binaryRefs", default)]
    pub binary_refs: Option<u8>,
}

impl BinaryRefQuery {
    pub fn enabled(&self) -> bool {
        self.binary_refs == Some(1)
    }
}

// ---------------------------------------------------------------------------
// Serialization helper
// ---------------------------------------------------------------------------

/// Serialize a chunk for an HTTP response.
///
/// Produces binary-ref SSE output for:
/// - `ContentType::Binary` when `?binaryRefs=1` (existing)
/// - `ContentType::BinaryRef` always (new)
///
/// All other chunk types are serialized in full.
pub fn serialize_chunk(chunk: &Chunk, binary_refs: bool) -> Value {
    let use_ref = (binary_refs && chunk.content_type == ContentType::Binary)
               || chunk.content_type == ContentType::BinaryRef;

    if use_ref {
        let byte_size: Option<u64> = if chunk.content_type == ContentType::Binary {
            chunk.data.as_ref().map(|d| d.len() as u64)
        } else {
            chunk.get_annotation::<u64>(keys::BINARY_BYTE_SIZE)
        };
        json!({
            "id":           chunk.id,
            "content_type": "binary-ref",
            "content": {
                "chunk_id":  chunk.id,
                "mime_type": chunk.mime_type,
                "byte_size": byte_size,
            },
            "data":         null,
            "mime_type":    null,
            "producer":     chunk.producer,
            "annotations":  chunk.annotations,
            "timestamp":    chunk.timestamp,
        })
    } else {
        serde_json::to_value(chunk).unwrap_or(Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_sdk::Chunk;

    fn binary_chunk() -> Chunk {
        Chunk::new_binary(
            vec![0u8, 1, 2, 3],
            "audio/mpeg",
            "com.nominal.cafe-tts",
        )
        .with_annotation("chat.role", "assistant")
    }

    fn text_chunk() -> Chunk {
        Chunk::new_text("hello", "com.nominal.cafe-llm")
            .with_annotation("chat.role", "assistant")
    }

    fn null_chunk() -> Chunk {
        Chunk::new_null("com.nominal.cafe-agent-runtime")
    }

    // binary chunk with binary_refs=true → binary-ref shape, no data field
    #[test]
    fn binary_chunk_with_refs_enabled() {
        let chunk = binary_chunk();
        let val = serialize_chunk(&chunk, true);

        assert_eq!(val["content_type"], "binary-ref");
        assert_eq!(val["content"]["chunk_id"], chunk.id.as_str());
        assert_eq!(val["content"]["mime_type"], "audio/mpeg");
        assert_eq!(val["content"]["byte_size"], 4);
        // data must be absent / null
        assert!(val["data"].is_null());
        // metadata preserved
        assert_eq!(val["producer"], "com.nominal.cafe-tts");
        assert_eq!(val["annotations"]["chat.role"], "assistant");
    }

    // binary chunk with binary_refs=false → full serialization with base64 data
    #[test]
    fn binary_chunk_with_refs_disabled() {
        let chunk = binary_chunk();
        let val = serialize_chunk(&chunk, false);

        assert_eq!(val["content_type"], "binary");
        // data field should be a non-null base64 string
        assert!(val["data"].is_string());
        assert!(val["content"]["chunk_id"].is_null());
    }

    // text chunk with binary_refs=true → unchanged (no substitution)
    #[test]
    fn text_chunk_passes_through_unchanged() {
        let chunk = text_chunk();
        let val_with = serialize_chunk(&chunk, true);
        let val_without = serialize_chunk(&chunk, false);

        assert_eq!(val_with["content_type"], "text");
        assert_eq!(val_with["content"], val_without["content"]);
    }

    // null chunk with binary_refs=true → unchanged
    #[test]
    fn null_chunk_passes_through_unchanged() {
        let chunk = null_chunk();
        let val = serialize_chunk(&chunk, true);
        assert_eq!(val["content_type"], "null");
    }

    // binary-ref chunk produces the same SSE output as stripped binary
    #[test]
    fn binary_ref_chunk_serializes_as_binary_ref() {
        let chunk = Chunk::new_binary_ref("audio/wav", "com.nominal.cafe-binary-store")
            .with_annotation("chat.role", "assistant");
        let val = serialize_chunk(&chunk, false); // flag doesn't matter for BinaryRef
        assert_eq!(val["content_type"], "binary-ref");
        assert_eq!(val["content"]["chunk_id"], chunk.id.as_str());
        assert_eq!(val["content"]["mime_type"], "audio/wav");
        assert!(val["data"].is_null());
        assert_eq!(val["annotations"]["chat.role"], "assistant");
    }

    #[test]
    fn binary_ref_chunk_with_byte_size() {
        use cafe_sdk::keys;
        let chunk = Chunk::new_binary_ref("audio/wav", "com.nominal.cafe-binary-store")
            .with_annotation(keys::BINARY_BYTE_SIZE, 1024u64);
        let val = serialize_chunk(&chunk, false);
        assert_eq!(val["content"]["byte_size"], 1024);
    }

    // byte_size is accurate
    #[test]
    fn byte_size_is_accurate() {
        let data = vec![42u8; 1024];
        let chunk = Chunk::new_binary(data, "audio/wav", "prod");
        let val = serialize_chunk(&chunk, true);
        assert_eq!(val["content"]["byte_size"], 1024);
    }
}
