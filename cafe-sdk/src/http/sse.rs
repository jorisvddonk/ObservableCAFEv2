use cafe_types::Chunk;

/// Parse one SSE `data: {...}\n` chunk from a byte buffer.
///
/// Removes the consumed bytes on success. Returns `None` if no complete SSE
/// chunk is available.
pub fn try_parse_sse_chunk(buffer: &mut String) -> Option<Chunk> {
    let data_prefix = "data: ";
    if let Some(start) = buffer.find(data_prefix) {
        let rest = &buffer[start + data_prefix.len()..];
        if let Some(end) = rest.find('\n') {
            let json_str = &rest[..end];
            let consumed = start + data_prefix.len() + end + 1;
            if let Ok(chunk) = serde_json::from_str::<Chunk>(json_str) {
                buffer.drain(..consumed);
                return Some(chunk);
            }
            buffer.drain(..consumed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_chunk() -> impl Strategy<Value = Chunk> {
        (
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            prop_oneof![
                Just(cafe_types::ContentType::Text),
                Just(cafe_types::ContentType::Binary),
                Just(cafe_types::ContentType::BinaryRef),
                Just(cafe_types::ContentType::Null),
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

    fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner.run(&strategy, |v| { test(v); Ok(()) }).unwrap();
    }

    #[test]
    fn parse_valid_sse() {
        run_proptest(arb_chunk(), |chunk: Chunk| {
            let json = serde_json::to_string(&chunk).unwrap();
            let sse = format!("data: {}\n", json);
            let mut buf = sse.clone();
            let parsed = try_parse_sse_chunk(&mut buf);
            assert!(parsed.is_some(), "failed to parse valid SSE: {}", sse);
            let parsed = parsed.unwrap();
            assert_eq!(parsed.id, chunk.id);
            assert_eq!(parsed.content_type, chunk.content_type);
            assert!(buf.is_empty(), "buffer not fully consumed: {:?}", buf);
        });
    }

    #[test]
    fn parse_valid_sse_with_prefix() {
        run_proptest(
            (".{0,20}", arb_chunk()),
            |(prefix, chunk): (String, Chunk)| {
                // Skip cases where "data: " appears within the prefix
                if prefix.contains("data: ") {
                    return;
                }
                let json = serde_json::to_string(&chunk).unwrap();
                let sse = format!("{}data: {}\n", prefix, json);
                let mut buf = sse.clone();
                let parsed = try_parse_sse_chunk(&mut buf);
                assert!(parsed.is_some());
                let parsed = parsed.unwrap();
                assert_eq!(parsed.id, chunk.id);
                // drain(..consumed) removes from position 0, so prefix is also consumed
                assert!(buf.len() < sse.len());
            },
        );
    }

    #[test]
    fn parse_incomplete_sse_returns_none() {
        run_proptest(arb_chunk(), |chunk: Chunk| {
            let json = serde_json::to_string(&chunk).unwrap();
            let incomplete = format!("data: {}", json);
            let mut buf = incomplete;
            let result = try_parse_sse_chunk(&mut buf);
            assert!(result.is_none());
        });
    }

    #[test]
    fn parse_no_prefix_returns_none() {
        run_proptest(".{0,100}", |text: String| {
            let mut buf = text.clone();
            let result = try_parse_sse_chunk(&mut buf);
            if !text.contains("data: ") || !text.contains('\n') {
                assert!(result.is_none());
            }
        });
    }

    #[test]
    fn parse_multiple_sse_first_consumed() {
        run_proptest(
            (arb_chunk(), arb_chunk()),
            |(chunk1, chunk2): (Chunk, Chunk)| {
                let json1 = serde_json::to_string(&chunk1).unwrap();
                let json2 = serde_json::to_string(&chunk2).unwrap();
                let sse = format!("data: {}\ndata: {}\n", json1, json2);
                let mut buf = sse.clone();
                let first = try_parse_sse_chunk(&mut buf);
                assert!(first.is_some());
                assert_eq!(first.unwrap().id, chunk1.id);
                let expected = format!("data: {}\n", json2);
                assert_eq!(buf, expected);
            },
        );
    }

    #[test]
    fn parse_never_panics() {
        run_proptest(".{0,200}", |any_bytes: String| {
            let mut buf = any_bytes;
            let _ = try_parse_sse_chunk(&mut buf);
        });
    }
}
