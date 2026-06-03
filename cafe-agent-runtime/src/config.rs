use cafe_types::{keys, Chunk, ContentType};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Runtime environment config (socket path, agent directories)
// ---------------------------------------------------------------------------

pub struct Config {
    pub socket_path: String,
    pub agent_paths: Vec<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let paths_str = std::env::var("CAFE_AGENT_PATHS").unwrap_or_else(|_| "./agents".into());
        let agent_paths = paths_str.split(':').map(String::from).collect();
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            agent_paths,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionConfig — fully merged runtime config for a session
// ---------------------------------------------------------------------------

/// Fully resolved, merged config for a session at a point in time.
/// All fields are Option<> because any field may be absent if never configured.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    // LLM evaluator
    pub llm_system_prompt: Option<String>,
    pub llm_temperature: Option<f32>,
    pub llm_max_tokens: Option<u32>,
    pub llm_model: Option<String>,
    pub llm_backend: Option<String>,

    // TTS evaluator
    pub tts_profile: Option<String>,
    pub tts_engine: Option<String>,
    pub tts_endpoint: Option<String>,

    // STT evaluator
    pub stt_base_url: Option<String>,
    pub stt_response_format: Option<String>,

    // RSS evaluator
    pub rss_url: Option<String>,

    /// Any extra config keys not explicitly modelled above.
    /// Key: full annotation key (e.g. "config.foo.bar"), value: JSON value.
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// resolve_session_config
// ---------------------------------------------------------------------------

/// Scan `history` in forward chronological order and merge all runtime config
/// null chunks into a single `SessionConfig`. Later chunks win per key.
///
/// This function is **pure** — no side effects, no caching. Evaluators must
/// call it on every activation so mid-session config updates take effect
/// immediately.
pub fn resolve_session_config(history: &[Chunk]) -> SessionConfig {
    let mut cfg = SessionConfig::default();

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

        // Merge every annotation key except the marker itself.
        for (key, value) in &chunk.annotations {
            if key == keys::CONFIG_TYPE {
                continue;
            }
            apply_config_key(&mut cfg, key, value);
        }
    }

    cfg
}

/// Map a single annotation key/value onto the appropriate `SessionConfig` field.
/// Unknown keys land in `extra` under their full key name.
fn apply_config_key(cfg: &mut SessionConfig, key: &str, value: &serde_json::Value) {
    match key {
        keys::CONFIG_LLM_SYSTEM_PROMPT => {
            cfg.llm_system_prompt = value.as_str().map(String::from);
        }
        keys::CONFIG_LLM_TEMPERATURE => {
            cfg.llm_temperature = value
                .as_f64()
                .map(|f| f as f32)
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
        }
        keys::CONFIG_LLM_MAX_TOKENS => {
            cfg.llm_max_tokens = value
                .as_u64()
                .map(|n| n as u32)
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
        }
        keys::CONFIG_LLM_MODEL => {
            cfg.llm_model = value.as_str().map(String::from);
        }
        keys::CONFIG_LLM_BACKEND => {
            cfg.llm_backend = value.as_str().map(String::from);
        }
        keys::CONFIG_TTS_PROFILE => {
            cfg.tts_profile = value.as_str().map(String::from);
        }
        keys::CONFIG_TTS_ENGINE => {
            cfg.tts_engine = value.as_str().map(String::from);
        }
        keys::CONFIG_TTS_ENDPOINT => {
            cfg.tts_endpoint = value.as_str().map(String::from);
        }
        keys::CONFIG_STT_BASE_URL => {
            cfg.stt_base_url = value.as_str().map(String::from);
        }
        keys::CONFIG_STT_RESPONSE_FORMAT => {
            cfg.stt_response_format = value.as_str().map(String::from);
        }
        keys::CONFIG_RSS_URL => {
            cfg.rss_url = value.as_str().map(String::from);
        }
        keys::CONFIG_SESSION_NAME => {
            // Special: also treated as a display-name update; stored in extra
            // so callers that care about session naming can read it.
            cfg.extra.insert(key.to_string(), value.clone());
        }
        _ => {
            cfg.extra.insert(key.to_string(), value.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// get_evaluator_config
// ---------------------------------------------------------------------------

/// Convenience for individual evaluators. Calls `resolve_session_config` and
/// returns only the keys whose annotation name starts with
/// `"config.<namespace>."`, stripped of that prefix.
///
/// Example: `get_evaluator_config(history, "tts")` on a history containing
/// `"config.tts.profile": "Volition"` returns `{ "profile": "Volition" }`.
pub fn get_evaluator_config(
    history: &[Chunk],
    namespace: &str,
) -> HashMap<String, serde_json::Value> {
    let cfg = resolve_session_config(history);
    let prefix = format!("config.{}.", namespace);

    // Collect known typed fields back into the map via extra + direct fields.
    // The simplest approach: re-scan history for only this namespace's keys.
    let mut result = HashMap::new();

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
        for (key, value) in &chunk.annotations {
            if let Some(suffix) = key.strip_prefix(&prefix) {
                result.insert(suffix.to_string(), value.clone());
            }
        }
    }

    // Also include anything that landed in `cfg.extra` for this namespace.
    for (key, value) in &cfg.extra {
        if let Some(suffix) = key.strip_prefix(&prefix) {
            result.insert(suffix.to_string(), value.clone());
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_types::Chunk;

    fn make_runtime_config_chunk(annotations: &[(&str, serde_json::Value)]) -> Chunk {
        let mut chunk = Chunk::new_null("test");
        chunk = chunk.with_annotation(keys::CONFIG_TYPE, "runtime");
        for (k, v) in annotations {
            chunk = chunk.with_annotation(*k, v.clone());
        }
        chunk
    }

    // 1. Empty history → all fields None, empty extra.
    #[test]
    fn empty_history_returns_all_none() {
        let cfg = resolve_session_config(&[]);
        assert!(cfg.llm_system_prompt.is_none());
        assert!(cfg.llm_temperature.is_none());
        assert!(cfg.llm_max_tokens.is_none());
        assert!(cfg.llm_model.is_none());
        assert!(cfg.llm_backend.is_none());
        assert!(cfg.tts_profile.is_none());
        assert!(cfg.tts_engine.is_none());
        assert!(cfg.tts_endpoint.is_none());
        assert!(cfg.stt_base_url.is_none());
        assert!(cfg.stt_response_format.is_none());
        assert!(cfg.rss_url.is_none());
        assert!(cfg.extra.is_empty());
    }

    // 2. Single config chunk sets one field, rest remain None.
    #[test]
    fn single_config_chunk_sets_system_prompt() {
        let chunk = make_runtime_config_chunk(&[(
            keys::CONFIG_LLM_SYSTEM_PROMPT,
            serde_json::Value::String("Hello".into()),
        )]);
        let cfg = resolve_session_config(&[chunk]);
        assert_eq!(cfg.llm_system_prompt, Some("Hello".into()));
        assert!(cfg.llm_temperature.is_none());
        assert!(cfg.llm_model.is_none());
        assert!(cfg.extra.is_empty());
    }

    // 3. Two chunks — second sets system_prompt; first also set temperature.
    //    Result must have BOTH (partial update, not a wipe).
    #[test]
    fn two_chunks_partial_update_preserves_earlier_keys() {
        let chunk1 = make_runtime_config_chunk(&[
            (
                keys::CONFIG_LLM_SYSTEM_PROMPT,
                serde_json::Value::String("A".into()),
            ),
            (
                keys::CONFIG_LLM_TEMPERATURE,
                serde_json::json!(0.5_f64),
            ),
        ]);
        let chunk2 = make_runtime_config_chunk(&[(
            keys::CONFIG_LLM_SYSTEM_PROMPT,
            serde_json::Value::String("B".into()),
        )]);

        let cfg = resolve_session_config(&[chunk1, chunk2]);
        assert_eq!(cfg.llm_system_prompt, Some("B".into())); // overridden
        assert!(
            (cfg.llm_temperature.unwrap() - 0.5).abs() < 1e-5,
            "temperature should survive the second chunk"
        );
    }

    // 4. Non-config chunks interspersed are ignored; result is same as without them.
    #[test]
    fn non_config_chunks_are_ignored() {
        let text_chunk = Chunk::new_text("user message", "user");
        let config_chunk = make_runtime_config_chunk(&[(
            keys::CONFIG_LLM_MODEL,
            serde_json::Value::String("gemma3:1b".into()),
        )]);
        let another_text = Chunk::new_text("assistant reply", "llm");

        let cfg = resolve_session_config(&[text_chunk, config_chunk, another_text]);
        assert_eq!(cfg.llm_model, Some("gemma3:1b".into()));
        // Everything else still None
        assert!(cfg.llm_system_prompt.is_none());
        assert!(cfg.extra.is_empty());
    }

    // 5. Unknown key ends up in extra with its full key name.
    #[test]
    fn unknown_key_goes_to_extra() {
        let chunk = make_runtime_config_chunk(&[(
            "config.foo.bar",
            serde_json::Value::String("baz".into()),
        )]);
        let cfg = resolve_session_config(&[chunk]);
        assert_eq!(
            cfg.extra.get("config.foo.bar"),
            Some(&serde_json::Value::String("baz".into()))
        );
    }

    // 6. get_evaluator_config strips the namespace prefix.
    #[test]
    fn get_evaluator_config_strips_prefix() {
        let chunk = make_runtime_config_chunk(&[
            (
                keys::CONFIG_TTS_PROFILE,
                serde_json::Value::String("Volition".into()),
            ),
            (
                keys::CONFIG_TTS_ENGINE,
                serde_json::Value::String("qwen".into()),
            ),
            (
                keys::CONFIG_LLM_MODEL,
                serde_json::Value::String("gemma3:1b".into()),
            ),
        ]);

        let tts = get_evaluator_config(&[chunk], "tts");
        assert_eq!(
            tts.get("profile"),
            Some(&serde_json::Value::String("Volition".into()))
        );
        assert_eq!(
            tts.get("engine"),
            Some(&serde_json::Value::String("qwen".into()))
        );
        // LLM key must not appear in TTS map
        assert!(!tts.contains_key("model"));
        assert!(!tts.contains_key("config.llm.model"));
    }
}
