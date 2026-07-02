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
