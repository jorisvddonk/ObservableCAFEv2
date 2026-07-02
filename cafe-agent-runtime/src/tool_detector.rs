use cafe_types::ToolCall;
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
}
