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
                messages.push(LlmMessage {
                    role: chunk.role().unwrap().into(),
                    content: chunk.content.clone().unwrap_or_default(),
                });
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
}
