use crate::backends::LlmMessage;
use cafe_sdk::{keys, Chunk, ContentType};

/// Build an LLM message list from session history.
pub fn build_messages(history: &[Chunk], system_prompt: Option<&str>) -> Vec<LlmMessage> {
    let mut messages = Vec::new();

    if let Some(prompt) = system_prompt {
        messages.push(LlmMessage {
            role: "system".into(),
            content: prompt.into(),
        });
    }

    for chunk in history {
        if chunk.content_type != ContentType::Text {
            continue;
        }

        // Skip untrusted chunks
        if let Some(trust) = chunk.get_annotation::<serde_json::Value>("security.trust-level") {
            if trust["trusted"] == serde_json::Value::Bool(false) {
                continue;
            }
        }

        match chunk.role() {
            Some("user") | Some("assistant") => {
                let role = chunk.role().unwrap().to_string();
                let content = chunk.content.clone().unwrap_or_default();
                // Coalesce consecutive messages with the same role: many backends
                // require strict user/assistant alternation and reject runs of
                // identical roles (e.g. duplicate assistant outputs or a burst of
                // unanswered user messages).
                if let Some(last) = messages.last_mut() {
                    if last.role == role {
                        last.content.push('\n');
                        last.content.push_str(&content);
                        continue;
                    }
                }
                messages.push(LlmMessage { role, content });
            }
            _ => {}
        }
    }

    messages
}

/// Resolved LLM config derived from a session's chunk history.
pub struct LlmConfig {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Compaction mode: "none" (default), "truncate", "summarize".
    /// See ADR-125.
    pub compaction_mode: Option<String>,
    /// Max user+assistant messages sent to the backend (system excluded).
    pub max_history_messages: Option<u32>,
    /// Max summed chars over user+assistant messages sent to the backend.
    pub max_history_chars: Option<u32>,
}

/// Result of applying compaction to a built message list.
pub struct CompactionInfo {
    pub dropped_messages: usize,
    pub dropped_chars: usize,
    /// The dropped prefix messages (oldest first), retained so `summarize`
    /// mode can condense them. Empty when nothing was dropped.
    pub dropped: Vec<LlmMessage>,
}

/// Compact a built message list oldest-first to fit the configured budgets.
///
/// - `messages[0]` with role `system` is never counted or dropped.
/// - Applies only when `compaction_mode` is `"truncate"` or `"summarize"`.
/// - `"none"` (or unset/invalid mode, or no budgets set) returns the input
///   unchanged.
/// - Message-count and char budgets both apply; whichever hits first wins.
/// - The newest message is never dropped, even if it alone exceeds the char
///   budget.
pub fn apply_compaction(messages: Vec<LlmMessage>, cfg: &LlmConfig) -> (Vec<LlmMessage>, CompactionInfo) {
    let empty = |messages| (messages, CompactionInfo { dropped_messages: 0, dropped_chars: 0, dropped: Vec::new() });
    let mode = cfg.compaction_mode.as_deref().unwrap_or("none");
    if mode != "truncate" && mode != "summarize" {
        return empty(messages);
    }
    let max_messages = cfg.max_history_messages.filter(|&n| n >= 1).map(|n| n as usize);
    let max_chars = cfg.max_history_chars.filter(|&n| n >= 1).map(|n| n as usize);
    if max_messages.is_none() && max_chars.is_none() {
        return empty(messages);
    }

    // Split off a leading system message; it is preserved verbatim.
    let (system, rest) = match messages.split_first() {
        Some((first, _)) if first.role == "system" => {
            let mut it = messages.into_iter();
            let sys = it.next();
            (sys.into_iter().collect::<Vec<_>>(), it.collect::<Vec<_>>())
        }
        _ => (Vec::new(), messages),
    };
    if rest.is_empty() {
        let mut out = system;
        out.extend(rest);
        return (out, CompactionInfo { dropped_messages: 0, dropped_chars: 0, dropped: Vec::new() });
    }

    // Oldest-first: find the smallest suffix of `rest` satisfying both budgets,
    // always keeping at least the newest message.
    let mut start = 0;
    while start < rest.len() {
        let window = &rest[start..];
        let count_ok = max_messages.map(|m| window.len() <= m).unwrap_or(true);
        let chars: usize = window.iter().map(|m| m.content.len()).sum();
        let chars_ok = max_chars.map(|c| chars <= c).unwrap_or(true);
        if (count_ok && chars_ok) || window.len() <= 1 {
            break;
        }
        start += 1;
    }

    let dropped_messages: usize = start;
    let dropped_chars: usize = rest[..start].iter().map(|m| m.content.len()).sum();
    let dropped: Vec<LlmMessage> = rest[..start].to_vec();
    let mut out = system;
    out.extend(rest.into_iter().skip(start));
    (out, CompactionInfo { dropped_messages, dropped_chars, dropped })
}

/// Build a summarization request for a dropped compaction prefix.
///
/// Returns `[system, user]` messages for a non-streaming completion call.
/// The caller sends this to the backend and inserts the resulting text via
/// `insert_summary`. Prompt-only: nothing is persisted to session history.
pub fn build_summary_request(dropped: &[LlmMessage]) -> Vec<LlmMessage> {
    let mut transcript = String::new();
    for m in dropped {
        transcript.push_str(&m.role);
        transcript.push_str(": ");
        transcript.push_str(&m.content);
        transcript.push('\n');
    }
    vec![
        LlmMessage {
            role: "system".into(),
            content: "Summarize the following conversation prefix concisely. Preserve key facts, decisions, user preferences, and open questions. Reply with the summary only, no preamble.".into(),
        },
        LlmMessage { role: "user".into(), content: transcript },
    ]
}

/// Insert a compaction summary into a compacted message list.
///
/// Placed right after a leading `system` message when present, otherwise
/// first. The summary is prompt-only (never written back to history).
pub fn insert_summary(mut messages: Vec<LlmMessage>, summary: &str) -> Vec<LlmMessage> {
    let summary_msg = LlmMessage {
        role: "system".into(),
        content: format!("Prior conversation summary (compacted):\n{summary}"),
    };
    let pos = match messages.first() {
        Some(first) if first.role == "system" => 1,
        _ => 0,
    };
    messages.insert(pos, summary_msg);
    messages
}

/// Extract LLM config by scanning history in **forward chronological order**,
/// merging all `config.type: runtime` null chunks on a per-key basis.
/// Later chunks win per key; a chunk that only sets one key does not wipe
/// the others (partial update semantics).
///
/// Reads the namespaced `config.llm.*` annotation keys as the canonical source.
/// Falls back to the legacy flat keys (`config.model`, etc.) so existing sessions
/// that were seeded with the old format continue to work.
pub fn extract_config(history: &[Chunk]) -> LlmConfig {
    let mut cfg = LlmConfig {
        backend: None,
        model: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
        compaction_mode: None,
        max_history_messages: None,
        max_history_chars: None,
    };

    for chunk in history {
        if chunk.content_type != ContentType::Null {
            continue;
        }
        if chunk
            .annotations
            .get(keys::CONFIG_TYPE)
            .and_then(|v| v.as_str())
            != Some("runtime")
        {
            continue;
        }

        // Namespaced keys (preferred, config.llm.*)
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_BACKEND) {
            cfg.backend = v.as_str().map(String::from);
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_MODEL) {
            cfg.model = v.as_str().map(String::from);
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_SYSTEM_PROMPT) {
            cfg.system_prompt = v.as_str().map(String::from);
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_TEMPERATURE) {
            cfg.temperature = v
                .as_f64()
                .map(|f| f as f32)
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_MAX_TOKENS) {
            cfg.max_tokens = v
                .as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_COMPACTION_MODE) {
            cfg.compaction_mode = v.as_str().map(String::from);
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_MAX_HISTORY_MESSAGES) {
            cfg.max_history_messages = v
                .as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        }
        if let Some(v) = chunk.annotations.get(keys::CONFIG_LLM_MAX_HISTORY_CHARS) {
            cfg.max_history_chars = v
                .as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        }

        // Legacy flat keys (config.backend, config.model, …) — kept for
        // backwards compatibility with sessions seeded before the namespace change.
        // Only applied when the namespaced key hasn't already set the field this chunk.
        if cfg.backend.is_none() {
            if let Some(v) = chunk.annotations.get(keys::CONFIG_BACKEND) {
                cfg.backend = v.as_str().map(String::from);
            }
        }
        if cfg.model.is_none() {
            if let Some(v) = chunk.annotations.get(keys::CONFIG_MODEL) {
                cfg.model = v.as_str().map(String::from);
            }
        }
        if cfg.system_prompt.is_none() {
            if let Some(v) = chunk.annotations.get(keys::CONFIG_SYSTEM_PROMPT) {
                cfg.system_prompt = v.as_str().map(String::from);
            }
        }
        if cfg.temperature.is_none() {
            if let Some(v) = chunk.annotations.get(keys::CONFIG_TEMPERATURE) {
                cfg.temperature = v
                    .as_f64()
                    .map(|f| f as f32)
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
            }
        }
        if cfg.max_tokens.is_none() {
            if let Some(v) = chunk.annotations.get(keys::CONFIG_MAX_TOKENS) {
                cfg.max_tokens = v
                    .as_u64()
                    .map(|n| n as u32)
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
            }
        }
    }

    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_annotation_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn arb_chunk() -> impl Strategy<Value = Chunk> {
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
            prop::collection::hash_map("[a-z._-]{1,15}", arb_annotation_value(), 0..5),
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

    fn arb_chunk_list() -> impl Strategy<Value = Vec<Chunk>> {
        prop::collection::vec(arb_chunk(), 0..30)
    }

    fn arb_runtime_config_chunk() -> impl Strategy<Value = Chunk> {
        (
            proptest::option::of(".{0,50}"),
            proptest::option::of(".{0,50}"),
            proptest::option::of(".{0,100}"),
            proptest::option::of(any::<f64>().prop_filter("finite", |f| f.is_finite())),
            proptest::option::of(any::<u32>()),
        )
            .prop_map(|(backend, model, system_prompt, temperature, max_tokens)| {
                let mut c = Chunk::new_null("test")
                    .with_annotation(keys::CONFIG_TYPE, "runtime");
                if let Some(v) = backend {
                    c = c.with_annotation(keys::CONFIG_LLM_BACKEND, v);
                }
                if let Some(v) = model {
                    c = c.with_annotation(keys::CONFIG_LLM_MODEL, v);
                }
                if let Some(v) = system_prompt {
                    c = c.with_annotation(keys::CONFIG_LLM_SYSTEM_PROMPT, v);
                }
                if let Some(v) = temperature {
                    c = c.with_annotation(keys::CONFIG_LLM_TEMPERATURE, v);
                }
                if let Some(v) = max_tokens {
                    c = c.with_annotation(keys::CONFIG_LLM_MAX_TOKENS, v);
                }
                c
            })
    }

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner
            .run(&strategy, |v| {
                test(v);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn build_messages_system_prompt_first() {
        run_proptest(
            (arb_chunk_list(), ".{0,50}".prop_map(Some)),
            |(history, prompt): (Vec<Chunk>, Option<String>)| {
                let messages = build_messages(&history, prompt.as_deref());
                if !messages.is_empty() {
                    assert_eq!(messages[0].role, "system");
                    if let Some(ref p) = prompt {
                        assert_eq!(messages[0].content, p.as_str());
                    }
                }
            },
        );
    }

    #[test]
    fn build_messages_filters_non_text() {
        run_proptest(arb_chunk_list(), |history: Vec<Chunk>| {
            let messages = build_messages(&history, None);
            let text_chunks: Vec<_> = history
                .iter()
                .filter(|c| c.content_type == ContentType::Text)
                .collect();
            for msg in &messages {
                assert!(msg.role == "user" || msg.role == "assistant");
                let matching_chunks: Vec<_> = text_chunks
                    .iter()
                    .filter(|c| {
                        c.role() == Some(msg.role.as_str())
                            && c.content.as_deref() == Some(msg.content.as_str())
                    })
                    .collect();
                assert!(
                    !matching_chunks.is_empty(),
                    "no chunk matched message role={} content={:?}",
                    msg.role,
                    msg.content
                );
            }
        });
    }

    #[test]
    fn build_messages_no_system_without_prompt() {
        run_proptest(arb_chunk_list(), |history: Vec<Chunk>| {
            let messages = build_messages(&history, None);
            for msg in &messages {
                assert_ne!(msg.role, "system", "no system prompt was provided");
            }
        });
    }

    #[test]
    fn build_messages_coalesces_consecutive_same_role() {
        let history = vec![
            Chunk::new_text("first", "producer").with_annotation(keys::CHAT_ROLE, "user"),
            Chunk::new_text("second", "producer").with_annotation(keys::CHAT_ROLE, "user"),
            Chunk::new_text("assistant text", "producer").with_annotation(keys::CHAT_ROLE, "assistant"),
            Chunk::new_text("third", "producer").with_annotation(keys::CHAT_ROLE, "user"),
            Chunk::new_text("dup", "producer").with_annotation(keys::CHAT_ROLE, "assistant"),
            Chunk::new_text("dup2", "producer").with_annotation(keys::CHAT_ROLE, "assistant"),
        ];
        let messages = build_messages(&history, None);
        let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
        assert_eq!(messages[0].content, "first\nsecond");
        assert_eq!(messages[3].content, "dup\ndup2");
    }

    #[test]
    fn extract_config_idempotent() {
        run_proptest(arb_chunk_list(), |chunks: Vec<Chunk>| {
            let cfg1 = extract_config(&chunks);
            let cfg2 = extract_config(&chunks);
            assert_eq!(cfg1.backend, cfg2.backend);
            assert_eq!(cfg1.model, cfg2.model);
            assert_eq!(cfg1.system_prompt, cfg2.system_prompt);
        });
    }

    #[test]
    fn extract_config_namespaced_takes_priority() {
        run_proptest(arb_chunk_list(), |mut history: Vec<Chunk>| {
            let chunk = Chunk::new_null("test")
                .with_annotation(keys::CONFIG_TYPE, "runtime")
                .with_annotation(keys::CONFIG_LLM_MODEL, "namespaced-model")
                .with_annotation(keys::CONFIG_MODEL, "legacy-model");
            history.push(chunk);
            let cfg = extract_config(&history);
            assert_eq!(cfg.model, Some("namespaced-model".into()));
        });
    }

    #[test]
    fn extract_config_system_prompt_from_legacy() {
        run_proptest(arb_chunk_list(), |mut history: Vec<Chunk>| {
            let chunk = Chunk::new_null("test")
                .with_annotation(keys::CONFIG_TYPE, "runtime")
                .with_annotation(keys::CONFIG_SYSTEM_PROMPT, "legacy-prompt");
            history.push(chunk);
            let cfg = extract_config(&history);
            assert_eq!(cfg.system_prompt, Some("legacy-prompt".into()));
        });
    }

    #[test]
    fn extract_config_ignores_non_config() {
        run_proptest(arb_chunk_list(), |mut chunks: Vec<Chunk>| {
            let config_chunk = Chunk::new_null("test")
                .with_annotation(keys::CONFIG_TYPE, "runtime")
                .with_annotation(keys::CONFIG_LLM_MODEL, "gemma3:1b");
            chunks.push(config_chunk);
            let cfg = extract_config(&chunks);
            assert_eq!(cfg.model, Some("gemma3:1b".into()));
        });
    }

    fn test_cfg(mode: Option<&str>, max_messages: Option<u32>, max_chars: Option<u32>) -> LlmConfig {
        LlmConfig {
            backend: None,
            model: None,
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            compaction_mode: mode.map(String::from),
            max_history_messages: max_messages,
            max_history_chars: max_chars,
        }
    }

    fn test_messages() -> Vec<LlmMessage> {
        vec![
            LlmMessage { role: "system".into(), content: "sys".into() },
            LlmMessage { role: "user".into(), content: "aaa".into() },
            LlmMessage { role: "assistant".into(), content: "bbb".into() },
            LlmMessage { role: "user".into(), content: "ccc".into() },
        ]
    }

    #[test]
    fn compaction_disabled_by_default() {
        let (out, info) = apply_compaction(test_messages(), &test_cfg(None, Some(1), Some(1)));
        assert_eq!(out.len(), 4);
        assert_eq!(info.dropped_messages, 0);
    }

    #[test]
    fn compaction_none_mode_is_identity() {
        let (out, info) = apply_compaction(test_messages(), &test_cfg(Some("none"), Some(1), Some(1)));
        assert_eq!(out.len(), 4);
        assert_eq!(info.dropped_messages, 0);
    }

    #[test]
    fn compaction_truncate_message_count() {
        let (out, info) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), Some(2), None));
        assert_eq!(out.len(), 3); // system + last 2
        assert_eq!(out[0].role, "system");
        assert_eq!(out[1].content, "bbb");
        assert_eq!(out[2].content, "ccc");
        assert_eq!(info.dropped_messages, 1);
        assert_eq!(info.dropped_chars, 3);
    }

    #[test]
    fn compaction_preserves_system_and_newest() {
        // Budget of 0 is invalid → treated as unset → identity.
        let (out, _) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), Some(0), None));
        assert_eq!(out.len(), 4);
        // Even a 1-char budget keeps the newest message alone.
        let (out, info) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), None, Some(1)));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[1].content, "ccc");
        assert_eq!(info.dropped_messages, 2);
    }

    #[test]
    fn compaction_char_budget() {
        // "aaa"+"bbb"+"ccc" = 9 chars; budget 6 keeps "bbb"+"ccc".
        let (out, info) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), None, Some(6)));
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].content, "bbb");
        assert_eq!(out[2].content, "ccc");
        assert_eq!(info.dropped_messages, 1);
    }

    #[test]
    fn compaction_both_budgets_first_hit_wins() {
        // Count allows 3 but chars allow only 1 → char budget wins.
        let (out, _) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), Some(3), Some(3)));
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].content, "ccc");
    }

    #[test]
    fn compaction_summarize_compacts_like_truncate() {
        // apply_compaction itself drops identically in both modes; the
        // summarize path additionally condenses `info.dropped` (see below).
        let (out_t, _) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), Some(1), None));
        let (out_s, info_s) = apply_compaction(test_messages(), &test_cfg(Some("summarize"), Some(1), None));
        assert_eq!(out_t, out_s);
        assert_eq!(info_s.dropped_messages, 2);
        assert_eq!(info_s.dropped.len(), 2);
        assert_eq!(info_s.dropped[0].content, "aaa");
        assert_eq!(info_s.dropped[1].content, "bbb");
    }

    #[test]
    fn compaction_dropped_is_empty_when_nothing_dropped() {
        let (_, info) = apply_compaction(test_messages(), &test_cfg(Some("truncate"), Some(10), None));
        assert_eq!(info.dropped_messages, 0);
        assert!(info.dropped.is_empty());
    }

    #[test]
    fn build_summary_request_covers_dropped_transcript() {
        let (_, info) = apply_compaction(test_messages(), &test_cfg(Some("summarize"), Some(1), None));
        let req = build_summary_request(&info.dropped);
        assert_eq!(req.len(), 2);
        assert_eq!(req[0].role, "system");
        assert_eq!(req[1].role, "user");
        assert!(req[1].content.contains("user: aaa"), "transcript must carry roles: {}", req[1].content);
        assert!(req[1].content.contains("assistant: bbb"), "transcript must carry roles: {}", req[1].content);
    }

    #[test]
    fn insert_summary_goes_after_system_prompt() {
        let (compacted, _) = apply_compaction(test_messages(), &test_cfg(Some("summarize"), Some(1), None));
        let out = insert_summary(compacted, "they discussed cats");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[0].content, "sys");
        assert_eq!(out[1].role, "system");
        assert!(out[1].content.contains("Prior conversation summary"), "{}", out[1].content);
        assert!(out[1].content.contains("they discussed cats"), "{}", out[1].content);
        assert_eq!(out[2].content, "ccc");
    }

    #[test]
    fn insert_summary_without_system_prompt_goes_first() {
        let msgs = vec![LlmMessage { role: "user".into(), content: "hi".into() }];
        let out = insert_summary(msgs, "s");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "system");
        assert!(out[0].content.contains("Prior conversation summary"));
        assert_eq!(out[1].content, "hi");
    }

    #[test]
    fn extract_config_reads_compaction_keys() {
        let chunk = Chunk::new_null("test")
            .with_annotation(keys::CONFIG_TYPE, "runtime")
            .with_annotation(keys::CONFIG_LLM_COMPACTION_MODE, "truncate")
            .with_annotation(keys::CONFIG_LLM_MAX_HISTORY_MESSAGES, 5u32)
            .with_annotation(keys::CONFIG_LLM_MAX_HISTORY_CHARS, 1000u32);
        let cfg = extract_config(&[chunk]);
        assert_eq!(cfg.compaction_mode, Some("truncate".into()));
        assert_eq!(cfg.max_history_messages, Some(5));
        assert_eq!(cfg.max_history_chars, Some(1000));
    }

    #[test]
    fn extract_config_compaction_later_wins() {
        let c1 = Chunk::new_null("test")
            .with_annotation(keys::CONFIG_TYPE, "runtime")
            .with_annotation(keys::CONFIG_LLM_MAX_HISTORY_MESSAGES, 5u32);
        let c2 = Chunk::new_null("test")
            .with_annotation(keys::CONFIG_TYPE, "runtime")
            .with_annotation(keys::CONFIG_LLM_MAX_HISTORY_MESSAGES, 2u32);
        let cfg = extract_config(&[c1, c2]);
        assert_eq!(cfg.max_history_messages, Some(2));
    }
}
