use cafe_sdk::{Chunk, ContentType};
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
/// When `binary_refs` is `true` and the chunk is binary, returns a lightweight
/// `binary-ref` object instead of the full base64 payload. All other chunk
/// types are serialized in full regardless of the flag.
pub fn serialize_chunk(chunk: &Chunk, binary_refs: bool) -> Value {
    if binary_refs && chunk.content_type == ContentType::Binary {
        json!({
            "id":           chunk.id,
            "content_type": "binary-ref",
            "content": {
                "chunk_id":  chunk.id,
                "mime_type": chunk.mime_type,
                "byte_size": chunk.data.as_ref().map(|d| d.len()).unwrap_or(0),
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

    // byte_size is accurate
    #[test]
    fn byte_size_is_accurate() {
        let data = vec![42u8; 1024];
        let chunk = Chunk::new_binary(data, "audio/wav", "prod");
        let val = serialize_chunk(&chunk, true);
        assert_eq!(val["content"]["byte_size"], 1024);
    }
}
