use cafe_sdk::ToolCall;
use regex::Regex;

/// Scan `text` for `<|tool_call|>...<|tool_call_end|>` markers.
///
/// Returns `(cleaned_text, detected_calls)` — the markers are stripped from
/// the text and each is parsed into a `ToolCall`.
pub fn detect(text: &str) -> (String, Vec<ToolCall>) {
    let re = match Regex::new(r"<\|tool_call\|>\s*(\{.*?\})\s*<\|tool_call_end\|>") {
        Ok(r) => r,
        Err(_) => return (text.to_string(), vec![]),
    };

    let mut calls = Vec::new();
    let mut cleaned = text.to_string();

    for cap in re.captures_iter(text) {
        let json_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        match serde_json::from_str::<ToolCall>(json_str) {
            Ok(call) => calls.push(call),
            Err(e) => {
                tracing::warn!("tool_detector: failed to parse tool call: {}", e);
            }
        }
    }

    if !calls.is_empty() {
        cleaned = re.replace_all(text, "").to_string();
        // Clean up extra whitespace from removed markers
        cleaned = cleaned.trim().to_string();
    }

    (cleaned, calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tool_calls() {
        let text = "Hello, how can I help you?";
        let (cleaned, calls) = detect(text);
        assert_eq!(cleaned, text);
        assert!(calls.is_empty());
    }

    #[test]
    fn single_tool_call() {
        let text = r#"Let me look that up.<|tool_call|>{"name":"sheetbot.list_tasks","parameters":{}}<|tool_call_end|>Give me a moment."#;
        let (cleaned, calls) = detect(text);
        assert_eq!(cleaned, "Let me look that up.Give me a moment.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "sheetbot.list_tasks");
    }

    #[test]
    fn multiple_tool_calls() {
        let text = concat!(
            r#"<|tool_call|>{"name":"sheetbot.list_sheets","parameters":{}}<|tool_call_end|>"#,
            r#"<|tool_call|>{"name":"sheetbot.list_tasks","parameters":{}}<|tool_call_end|>"#
        );
        let (cleaned, calls) = detect(text);
        assert_eq!(cleaned, "");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "sheetbot.list_sheets");
        assert_eq!(calls[1].name, "sheetbot.list_tasks");
    }

    #[test]
    fn tool_call_with_params() {
        let text = r#"<|tool_call|>{"name":"sheetbot.get_task","parameters":{"id":"abc-123"}}<|tool_call_end|>"#;
        let (_, calls) = detect(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "sheetbot.get_task");
        assert_eq!(calls[0].parameters["id"], "abc-123");
    }

    #[test]
    fn invalid_json_is_skipped() {
        let text = r"<|tool_call|>{bad json}<|tool_call_end|>";
        let (_, calls) = detect(text);
        assert!(calls.is_empty());
    }

    // ── Property-based tests (proptest) ──

    use proptest::prelude::*;

    fn arb_tool_call_json() -> impl Strategy<Value = String> {
        (".{1,30}", proptest::collection::hash_map(".{1,15}", arb_json_value(), 0..3))
            .prop_map(|(name, parameters)| {
                serde_json::json!({"name": name, "parameters": parameters}).to_string()
            })
    }

    fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn arb_text_with_call() -> impl Strategy<Value = String> {
        (".{0,50}", arb_tool_call_json(), ".{0,50}")
            .prop_map(|(before, call_json, after)| {
                format!("{}<|tool_call|>{}\n<|tool_call_end|>{}", before, call_json, after)
            })
    }

    fn arb_plain_text() -> impl Strategy<Value = String> {
        ".{0,100}".prop_map(|s| s)
    }

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner.run(&strategy, |v| { test(v); Ok(()) }).unwrap();
    }

    #[test]
    fn detect_never_panics() {
        run_proptest(arb_plain_text(), |text: String| {
            let (cleaned, calls) = detect(&text);
            assert!(cleaned.len() <= text.len() + 1);
            let _ = calls;
        });
    }

    #[test]
    fn detect_removes_markers() {
        run_proptest(arb_text_with_call(), |text: String| {
            let (cleaned, calls) = detect(&text);
            assert!(!cleaned.contains("<|tool_call|>"));
            assert!(!cleaned.contains("<|tool_call_end|>"));
            assert!(!calls.is_empty());
        });
    }

    #[test]
    fn detect_valid_json_parses() {
        run_proptest(arb_tool_call_json(), |call_json: String| {
            let text = format!("<|tool_call|>{}<|tool_call_end|>", call_json);
            let (_, calls) = detect(&text);
            assert!(!calls.is_empty());
            for call in &calls {
                assert!(!call.name.is_empty());
            }
        });
    }

    #[test]
    fn detect_plain_text_no_calls() {
        run_proptest(arb_plain_text(), |text: String| {
            let (cleaned, calls) = detect(&text);
            if !text.contains("<|tool_call|>") {
                assert!(calls.is_empty());
                assert_eq!(cleaned.trim(), text.trim());
            }
        });
    }

    #[test]
    fn detect_malformed_json_skipped() {
        run_proptest(
            ".{0,30}",
            |marker_text: String| {
                let text = format!("<|tool_call|>{}<|tool_call_end|>", marker_text);
                let (_, calls) = detect(&text);
                // should not panic
            },
        );
    }

    #[test]
    fn detect_handles_unclosed_marker() {
        run_proptest(
            ".{0,100}",
            |text: String| {
                let text = format!("{}<|tool_call|>", text);
                let (_, calls) = detect(&text);
                assert!(calls.is_empty());
            },
        );
    }

    #[test]
    fn detect_handles_empty_markers() {
        let (cleaned, calls) = detect("<|tool_call|><|tool_call_end|>");
        assert!(calls.is_empty());
        // Regex requires `{...}` between markers; empty markers are kept as-is
        assert!(cleaned.contains("<|tool_call|>") || cleaned.is_empty());
    }
}
