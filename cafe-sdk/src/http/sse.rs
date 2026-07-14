use cafe_types::Chunk;

/// Outcome of attempting to parse one SSE `data: {...}\n` frame from a buffer.
#[derive(Debug)]
pub enum SseParseOutcome {
    /// A complete, valid SSE frame was parsed and its bytes were consumed.
    Chunk(Chunk),
    /// A complete `data: ...` line was found and consumed, but its JSON body
    /// failed to parse. The malformed line is surfaced (rather than silently
    /// dropped) while still being consumed so forward progress is preserved and
    /// subsequent valid frames can be parsed.
    Invalid { raw: String, error: String },
    /// No complete SSE frame is currently available in the buffer.
    Incomplete,
}

/// Parse one SSE `data: {...}\n` chunk from a byte buffer.
///
/// On a valid frame the consumed bytes are removed and [`SseParseOutcome::Chunk`]
/// is returned. On a complete but unparseable frame the bytes are still consumed
/// but the error is surfaced via [`SseParseOutcome::Invalid`] instead of being
/// silently discarded. Returns [`SseParseOutcome::Incomplete`] when no complete
/// SSE chunk is available.
pub fn try_parse_sse_chunk(buffer: &mut String) -> SseParseOutcome {
    let data_prefix = "data: ";
    if let Some(start) = buffer.find(data_prefix) {
        let rest = &buffer[start + data_prefix.len()..];
        if let Some(end) = rest.find('\n') {
            let json_str = &rest[..end];
            let consumed = start + data_prefix.len() + end + 1;
            let raw = json_str.to_string();
            match serde_json::from_str::<Chunk>(json_str) {
                Ok(chunk) => {
                    buffer.drain(..consumed);
                    return SseParseOutcome::Chunk(chunk);
                }
                Err(e) => {
                    buffer.drain(..consumed);
                    return SseParseOutcome::Invalid {
                        raw,
                        error: e.to_string(),
                    };
                }
            }
        }
    }
    SseParseOutcome::Incomplete
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
            match try_parse_sse_chunk(&mut buf) {
                SseParseOutcome::Chunk(parsed) => {
                    assert_eq!(parsed.id, chunk.id);
                    assert_eq!(parsed.content_type, chunk.content_type);
                    assert!(buf.is_empty(), "buffer not fully consumed: {:?}", buf);
                }
                other => panic!("failed to parse valid SSE: {} -> {:?}", sse, other),
            }
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
                match try_parse_sse_chunk(&mut buf) {
                    SseParseOutcome::Chunk(parsed) => {
                        assert_eq!(parsed.id, chunk.id);
                        // drain(..consumed) removes from position 0, so prefix is also consumed
                        assert!(buf.len() < sse.len());
                    }
                    other => panic!("failed to parse valid SSE: {} -> {:?}", sse, other),
                }
            },
        );
    }

    #[test]
    fn parse_incomplete_sse_returns_incomplete() {
        run_proptest(arb_chunk(), |chunk: Chunk| {
            let json = serde_json::to_string(&chunk).unwrap();
            let incomplete = format!("data: {}", json);
            let mut buf = incomplete;
            assert!(matches!(
                try_parse_sse_chunk(&mut buf),
                SseParseOutcome::Incomplete
            ));
        });
    }

    #[test]
    fn parse_no_prefix_returns_incomplete() {
        run_proptest(".{0,100}", |text: String| {
            let mut buf = text.clone();
            if !text.contains("data: ") || !text.contains('\n') {
                assert!(matches!(
                    try_parse_sse_chunk(&mut buf),
                    SseParseOutcome::Incomplete
                ));
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
                match try_parse_sse_chunk(&mut buf) {
                    SseParseOutcome::Chunk(first) => {
                        assert_eq!(first.id, chunk1.id);
                        let expected = format!("data: {}\n", json2);
                        assert_eq!(buf, expected);
                    }
                    other => panic!("expected Chunk, got {:?}", other),
                }
            },
        );
    }

    /// Regression test for the silent-drop bug: a malformed `data:` line must
    /// be surfaced as `Invalid` (not silently dropped) AND a subsequent valid
    /// frame must still be parsed.
    #[test]
    fn parse_malformed_then_valid_reports_invalid_and_keeps_progress() {
        let good = Chunk::new_null("producer-x");
        let good_json = serde_json::to_string(&good).unwrap();
        let mut buf = format!("data: {{not valid json}}\ndata: {}\n", good_json);

        // First frame: malformed JSON -> surfaced as Invalid, not silently dropped.
        match try_parse_sse_chunk(&mut buf) {
            SseParseOutcome::Invalid { raw, error } => {
                assert_eq!(raw, "{not valid json}");
                assert!(!error.is_empty(), "error should be reported");
            }
            other => panic!("expected Invalid on malformed frame, got {:?}", other),
        }
        assert!(buf.contains(&good_json), "buffer should retain the valid frame");

        // Second frame: the valid line must still be parseable after the
        // malformed one was dropped.
        match try_parse_sse_chunk(&mut buf) {
            SseParseOutcome::Chunk(parsed) => {
                assert_eq!(parsed.id, good.id);
                assert_eq!(parsed.producer, "producer-x");
            }
            other => panic!("expected Chunk for valid frame, got {:?}", other),
        }
        assert!(buf.is_empty(), "buffer not fully consumed: {:?}", buf);
    }

    #[test]
    fn parse_never_panics() {
        run_proptest(".{0,200}", |any_bytes: String| {
            let mut buf = any_bytes;
            let _ = try_parse_sse_chunk(&mut buf);
        });
    }
}
