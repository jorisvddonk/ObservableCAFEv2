use cafe_sdk::{keys, Chunk, ContentType};
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
        let mut agent_paths = vec!["./agents".to_string()];
        if let Ok(paths_str) = std::env::var("ObservableCAFE_AGENT_SEARCH_PATHS")
            .or_else(|_| std::env::var("CAFE_AGENT_PATHS"))
        {
            agent_paths.extend(paths_str.split(':').map(String::from));
        }
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

    // ComfyUI evaluator
    pub comfy_workflow_path: Option<String>,
    pub comfy_workflow_input_node: Option<String>,
    pub comfy_endpoint: Option<String>,

    // SheetBot integration
    pub sheetbot_url: Option<String>,
    pub sheetbot_api_key: Option<String>,

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
        keys::CONFIG_COMFY_WORKFLOW_PATH => {
            cfg.comfy_workflow_path = value.as_str().map(String::from);
        }
        keys::CONFIG_COMFY_WORKFLOW_INPUT_NODE => {
            cfg.comfy_workflow_input_node = value.as_str().map(String::from);
        }
        keys::CONFIG_COMFY_ENDPOINT => {
            cfg.comfy_endpoint = value.as_str().map(String::from);
        }
        keys::CONFIG_SHEETBOT_URL => {
            cfg.sheetbot_url = value.as_str().map(String::from);
        }
        keys::CONFIG_SHEETBOT_API_KEY => {
            cfg.sheetbot_api_key = value.as_str().map(String::from);
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
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_sdk::Chunk;

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
        assert!(cfg.comfy_workflow_path.is_none());
        assert!(cfg.comfy_workflow_input_node.is_none());
        assert!(cfg.comfy_endpoint.is_none());
        assert!(cfg.sheetbot_url.is_none());
        assert!(cfg.sheetbot_api_key.is_none());
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
            (keys::CONFIG_LLM_TEMPERATURE, serde_json::json!(0.5_f64)),
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

    // ── Property-based tests (proptest) ──

    use proptest::prelude::*;

    fn arb_annotation_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn arb_annotation_map() -> impl Strategy<Value = Vec<(String, serde_json::Value)>> {
        prop::collection::vec(
            ("[a-z._-]{1,20}".prop_map(|s| format!("config.{}", s)), arb_annotation_value()),
            0..10,
        )
    }

    fn arb_runtime_config_chunk() -> impl Strategy<Value = Chunk> {
        arb_annotation_map().prop_map(|annotations| {
            let mut chunk = Chunk::new_null("proptest");
            chunk = chunk.with_annotation(keys::CONFIG_TYPE, "runtime");
            for (k, v) in annotations {
                chunk = chunk.with_annotation(k, v);
            }
            chunk
        })
    }

    fn arb_any_chunk() -> impl Strategy<Value = Chunk> {
        prop_oneof![
            arb_runtime_config_chunk().boxed(),
            (0..100).prop_map(|_| Chunk::new_text("hello", "test")).boxed(),
            (0..100).prop_map(|_| Chunk::new_binary(vec![1, 2, 3], "audio/wav", "test")).boxed(),
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
    fn prop_empty_history_all_none() {
        let cfg = resolve_session_config(&[]);
        assert!(cfg.llm_system_prompt.is_none());
        assert!(cfg.llm_temperature.is_none());
        assert!(cfg.llm_max_tokens.is_none());
        assert!(cfg.llm_model.is_none());
        assert!(cfg.llm_backend.is_none());
        assert!(cfg.tts_profile.is_none());
        assert!(cfg.tts_engine.is_none());
        assert!(cfg.tts_endpoint.is_none());
        assert!(cfg.comfy_workflow_path.is_none());
        assert!(cfg.comfy_workflow_input_node.is_none());
        assert!(cfg.comfy_endpoint.is_none());
        assert!(cfg.sheetbot_url.is_none());
        assert!(cfg.sheetbot_api_key.is_none());
        assert!(cfg.stt_base_url.is_none());
        assert!(cfg.stt_response_format.is_none());
        assert!(cfg.rss_url.is_none());
        assert!(cfg.extra.is_empty());
    }

    #[test]
    fn prop_idempotent() {
        run_proptest(
            prop::collection::vec(arb_any_chunk(), 0..20),
            |chunks: Vec<Chunk>| {
                let cfg1 = resolve_session_config(&chunks);
                let cfg2 = resolve_session_config(&chunks);
                assert_eq!(cfg1.llm_system_prompt, cfg2.llm_system_prompt);
                assert_eq!(cfg1.llm_temperature, cfg2.llm_temperature);
                assert_eq!(cfg1.llm_max_tokens, cfg2.llm_max_tokens);
                assert_eq!(cfg1.llm_model, cfg2.llm_model);
                assert_eq!(cfg1.llm_backend, cfg2.llm_backend);
                assert_eq!(cfg1.tts_profile, cfg2.tts_profile);
                assert_eq!(cfg1.comfy_workflow_path, cfg2.comfy_workflow_path);
                assert_eq!(cfg1.sheetbot_url, cfg2.sheetbot_url);
                assert_eq!(cfg1.stt_base_url, cfg2.stt_base_url);
                assert_eq!(cfg1.rss_url, cfg2.rss_url);
            },
        );
    }

    #[test]
    fn prop_partial_update_preserves_unset_fields() {
        run_proptest(
            prop::collection::vec(arb_runtime_config_chunk(), 0..10),
            |chunks: Vec<Chunk>| {
                let cfg = resolve_session_config(&chunks);
                let mut twice = chunks.clone();
                twice.extend(chunks);
                let cfg2 = resolve_session_config(&twice);
                assert_eq!(cfg.llm_system_prompt, cfg2.llm_system_prompt);
                assert_eq!(cfg.llm_temperature, cfg2.llm_temperature);
                assert_eq!(cfg.llm_model, cfg2.llm_model);
            },
        );
    }

    #[test]
    fn prop_non_config_chunks_ignored() {
        run_proptest(
            prop::collection::vec(arb_any_chunk(), 0..20),
            |chunks: Vec<Chunk>| {
                let cfg = resolve_session_config(&chunks);
                // should not crash
            },
        );
    }

    #[test]
    fn prop_unknown_keys_go_to_extra() {
        run_proptest(arb_annotation_map(), |annotations: Vec<(String, serde_json::Value)>| {
            let mut chunk = Chunk::new_null("proptest");
            chunk = chunk.with_annotation(keys::CONFIG_TYPE, "runtime");
            for (k, v) in &annotations {
                chunk = chunk.with_annotation(k, v.clone());
            }
            let cfg = resolve_session_config(&[chunk]);
            assert!(cfg.extra.len() <= annotations.len());
        });
    }

    #[test]
    fn prop_later_chunks_override_earlier() {
        run_proptest(
            (
                prop::collection::vec(arb_runtime_config_chunk(), 0..5),
                prop::collection::vec(arb_runtime_config_chunk(), 0..5),
            ),
            |(chunks1, chunks2): (Vec<Chunk>, Vec<Chunk>)| {
                let mut combined = chunks1;
                combined.extend(chunks2);
                let _cfg = resolve_session_config(&combined);
            },
        );
    }

    // ── apply_config_key property tests ──

    fn known_config_keys() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(keys::CONFIG_LLM_SYSTEM_PROMPT.to_string()),
            Just(keys::CONFIG_LLM_TEMPERATURE.to_string()),
            Just(keys::CONFIG_LLM_MAX_TOKENS.to_string()),
            Just(keys::CONFIG_LLM_MODEL.to_string()),
            Just(keys::CONFIG_LLM_BACKEND.to_string()),
            Just(keys::CONFIG_TTS_PROFILE.to_string()),
            Just(keys::CONFIG_TTS_ENGINE.to_string()),
            Just(keys::CONFIG_TTS_ENDPOINT.to_string()),
            Just(keys::CONFIG_COMFY_WORKFLOW_PATH.to_string()),
            Just(keys::CONFIG_COMFY_WORKFLOW_INPUT_NODE.to_string()),
            Just(keys::CONFIG_COMFY_ENDPOINT.to_string()),
            Just(keys::CONFIG_SHEETBOT_URL.to_string()),
            Just(keys::CONFIG_SHEETBOT_API_KEY.to_string()),
            Just(keys::CONFIG_STT_BASE_URL.to_string()),
            Just(keys::CONFIG_STT_RESPONSE_FORMAT.to_string()),
            Just(keys::CONFIG_RSS_URL.to_string()),
        ]
    }

    #[test]
    fn apply_known_string_key_sets_field() {
        run_proptest(
            (known_config_keys(), ".{0,30}"),
            |(key, val): (String, String)| {
                let mut cfg = SessionConfig::default();
                apply_config_key(&mut cfg, &key, &serde_json::Value::String(val.clone()));
                // All known string keys set their corresponding Option<String>
                let field_value = match key.as_str() {
                    keys::CONFIG_LLM_SYSTEM_PROMPT => cfg.llm_system_prompt,
                    keys::CONFIG_LLM_MODEL => cfg.llm_model,
                    keys::CONFIG_LLM_BACKEND => cfg.llm_backend,
                    keys::CONFIG_TTS_PROFILE => cfg.tts_profile,
                    keys::CONFIG_TTS_ENGINE => cfg.tts_engine,
                    keys::CONFIG_TTS_ENDPOINT => cfg.tts_endpoint,
                    keys::CONFIG_COMFY_WORKFLOW_PATH => cfg.comfy_workflow_path,
                    keys::CONFIG_COMFY_WORKFLOW_INPUT_NODE => cfg.comfy_workflow_input_node,
                    keys::CONFIG_COMFY_ENDPOINT => cfg.comfy_endpoint,
                    keys::CONFIG_SHEETBOT_URL => cfg.sheetbot_url,
                    keys::CONFIG_SHEETBOT_API_KEY => cfg.sheetbot_api_key,
                    keys::CONFIG_STT_BASE_URL => cfg.stt_base_url,
                    keys::CONFIG_STT_RESPONSE_FORMAT => cfg.stt_response_format,
                    keys::CONFIG_RSS_URL => cfg.rss_url,
                    _ => return,
                };
                assert_eq!(field_value, Some(val));
            },
        );
    }

    #[test]
    fn apply_unknown_key_goes_to_extra() {
        run_proptest(
            ("config\\.\\w{1,15}", arb_annotation_value()),
            |(key, val): (String, serde_json::Value)| {
                // Skip keys that are known
                let known = [
                    keys::CONFIG_LLM_SYSTEM_PROMPT, keys::CONFIG_LLM_TEMPERATURE,
                    keys::CONFIG_LLM_MAX_TOKENS, keys::CONFIG_LLM_MODEL, keys::CONFIG_LLM_BACKEND,
                    keys::CONFIG_TTS_PROFILE, keys::CONFIG_TTS_ENGINE, keys::CONFIG_TTS_ENDPOINT,
                    keys::CONFIG_COMFY_WORKFLOW_PATH, keys::CONFIG_COMFY_WORKFLOW_INPUT_NODE,
                    keys::CONFIG_COMFY_ENDPOINT, keys::CONFIG_SHEETBOT_URL, keys::CONFIG_SHEETBOT_API_KEY,
                    keys::CONFIG_STT_BASE_URL, keys::CONFIG_STT_RESPONSE_FORMAT, keys::CONFIG_RSS_URL,
                ];
                if known.contains(&key.as_str()) { return; }
                let mut cfg = SessionConfig::default();
                apply_config_key(&mut cfg, &key, &val);
                assert_eq!(cfg.extra.get(&key), Some(&val));
            },
        );
    }

    #[test]
    fn apply_non_string_value_does_not_crash() {
        run_proptest(
            (known_config_keys(), arb_annotation_value()),
            |(key, val): (String, serde_json::Value)| {
                let mut cfg = SessionConfig::default();
                apply_config_key(&mut cfg, &key, &val);
                // Should not panic regardless of value type
            },
        );
    }

    #[test]
    fn apply_temperature_from_number() {
        run_proptest(
            any::<f32>().prop_filter("finite", |f| f.is_finite()),
            |temp_f32: f32| {
                let mut cfg = SessionConfig::default();
                apply_config_key(&mut cfg, keys::CONFIG_LLM_TEMPERATURE, &serde_json::json!(temp_f32));
                // f32 → serde_json → f64 → f32 round-trip should be exact for f32 values
                assert!((cfg.llm_temperature.unwrap() - temp_f32).abs() < 1e-6);
            },
        );
    }

    #[test]
    fn apply_temperature_from_string() {
        let mut cfg = SessionConfig::default();
        apply_config_key(&mut cfg, keys::CONFIG_LLM_TEMPERATURE, &serde_json::Value::String("0.7".into()));
        assert!((cfg.llm_temperature.unwrap() - 0.7).abs() < 1e-5);
    }
}
