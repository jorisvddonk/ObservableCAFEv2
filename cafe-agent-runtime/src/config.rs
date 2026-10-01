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
        if let Ok(paths_str) = std::env::var("CAFE_AGENT_SEARCH_PATHS")
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
///
/// This is deliberately schema-agnostic (ADR-121): instead of one typed field
/// per evaluator, it holds every merged annotation verbatim under its full
/// key. Adding an evaluator requires no changes here — its keys resolve
/// through `values`, and undeclared keys land in `extra` as before.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// Every merged runtime-config annotation: full key (e.g.
    /// `"config.tts.backend"`) → last value seen. Later chunks win.
    pub values: HashMap<String, serde_json::Value>,
    /// The subset of `values` not declared in any known evaluator schema.
    /// Key: full annotation key, value: JSON value.
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// resolve_session_config
// ---------------------------------------------------------------------------

/// Collect every full config annotation key declared by the given evaluator
/// schemas (`config_schema.properties`).
pub fn known_config_keys(schemas: &[cafe_sdk::EvaluatorSchema]) -> std::collections::HashSet<String> {
    let mut keys = std::collections::HashSet::new();
    for schema in schemas {
        if let Some(props) = schema.config_schema.get("properties").and_then(|p| p.as_object()) {
            keys.extend(props.keys().cloned());
        }
    }
    keys
}

/// Scan `history` in forward chronological order and merge all runtime config
/// null chunks into a single `SessionConfig`. Later chunks win per key.
///
/// `schemas` is the currently known evaluator schemas: keys they declare land
/// in `values` (and are therefore visible to schema-driven param building);
/// everything else lands in both `values` and `extra`, exactly as before.
///
/// This function is **pure** — no side effects, no caching. Callers must
/// invoke it on every activation so mid-session config updates take effect
/// immediately.
pub fn resolve_session_config(
    history: &[Chunk],
    schemas: &[cafe_sdk::EvaluatorSchema],
) -> SessionConfig {
    let known = known_config_keys(schemas);
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

        // Merge every annotation key except the marker itself, verbatim.
        // Values pass through untouched: schema-driven param building reads
        // them as JSON, so no per-evaluator coercion is needed here.
        for (key, value) in &chunk.annotations {
            if key == keys::CONFIG_TYPE {
                continue;
            }
            cfg.values.insert(key.clone(), value.clone());
            if !known.contains(key.as_str()) {
                cfg.extra.insert(key.clone(), value.clone());
            }
        }
    }

    cfg
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_sdk::{Chunk, EvaluatorSchema};

    fn make_runtime_config_chunk(annotations: &[(&str, serde_json::Value)]) -> Chunk {
        let mut chunk = Chunk::new_null("test");
        chunk = chunk.with_annotation(keys::CONFIG_TYPE, "runtime");
        for (k, v) in annotations {
            chunk = chunk.with_annotation(*k, v.clone());
        }
        chunk
    }

    /// A schema declaring a single config key (used to mark keys known).
    fn schema_with_key(evaluator: &str, key: &str) -> EvaluatorSchema {
        EvaluatorSchema {
            name: evaluator.into(),
            description: format!("{evaluator} evaluator"),
            config_schema: serde_json::json!({
                "type": "object",
                "properties": { key: { "type": "string" } },
            }),
            rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    // 1. Empty history → both maps empty.
    #[test]
    fn empty_history_returns_empty_maps() {
        let cfg = resolve_session_config(&[], &[]);
        assert!(cfg.values.is_empty());
        assert!(cfg.extra.is_empty());
    }

    // 2. Declared keys land in `values` (verbatim) and stay out of `extra`;
    //    undeclared keys land in both.
    #[test]
    fn declared_and_undeclared_keys_split() {
        let chunk = make_runtime_config_chunk(&[
            (keys::CONFIG_LLM_MODEL, serde_json::Value::String("gemma3:1b".into())),
            ("config.foo.bar", serde_json::Value::String("baz".into())),
            (keys::CONFIG_LLM_TEMPERATURE, serde_json::json!(0.5_f64)),
        ]);
        let schemas = vec![schema_with_key("llm", keys::CONFIG_LLM_MODEL)];
        let cfg = resolve_session_config(&[chunk], &schemas);
        assert_eq!(
            cfg.values.get(keys::CONFIG_LLM_MODEL),
            Some(&serde_json::Value::String("gemma3:1b".into()))
        );
        // Verbatim: numbers are not coerced, strings stay strings.
        assert_eq!(cfg.values.get(keys::CONFIG_LLM_TEMPERATURE), Some(&serde_json::json!(0.5_f64)));
        assert_eq!(
            cfg.values.get("config.foo.bar"),
            Some(&serde_json::Value::String("baz".into()))
        );
        // Declared key absent from extra; undeclared keys (and the
        // undeclared temperature) present.
        assert!(!cfg.extra.contains_key(keys::CONFIG_LLM_MODEL));
        assert_eq!(
            cfg.extra.get("config.foo.bar"),
            Some(&serde_json::Value::String("baz".into()))
        );
        assert_eq!(cfg.extra.get(keys::CONFIG_LLM_TEMPERATURE), Some(&serde_json::json!(0.5_f64)));
    }

    // 3. Later chunks win per key; untouched keys survive.
    #[test]
    fn later_chunks_win_per_key() {
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

        let cfg = resolve_session_config(&[chunk1, chunk2], &[]);
        assert_eq!(
            cfg.values.get(keys::CONFIG_LLM_SYSTEM_PROMPT),
            Some(&serde_json::Value::String("B".into()))
        );
        assert_eq!(cfg.values.get(keys::CONFIG_LLM_TEMPERATURE), Some(&serde_json::json!(0.5_f64)));
    }

    // 4. Non-config chunks interspersed are ignored.
    #[test]
    fn non_config_chunks_are_ignored() {
        let text_chunk = Chunk::new_text("user message", "user");
        let config_chunk = make_runtime_config_chunk(&[(
            keys::CONFIG_LLM_MODEL,
            serde_json::Value::String("gemma3:1b".into()),
        )]);
        let another_text = Chunk::new_text("assistant reply", "llm");

        let cfg = resolve_session_config(&[text_chunk, config_chunk, another_text], &[]);
        assert_eq!(
            cfg.values.get(keys::CONFIG_LLM_MODEL),
            Some(&serde_json::Value::String("gemma3:1b".into()))
        );
        assert_eq!(cfg.values.len(), 1);
    }

    // 5. Without schemas every key is undeclared → values == extra.
    #[test]
    fn without_schemas_everything_is_extra() {
        let chunk = make_runtime_config_chunk(&[(
            keys::CONFIG_LLM_MODEL,
            serde_json::Value::String("gemma3:1b".into()),
        )]);
        let cfg = resolve_session_config(&[chunk], &[]);
        assert_eq!(cfg.values, cfg.extra);
    }

    // 6. known_config_keys unions schema properties deterministically.
    #[test]
    fn known_keys_union() {
        let schemas = vec![
            schema_with_key("b-eval", "config.b.x"),
            schema_with_key("a-eval", "config.a.x"),
            EvaluatorSchema {
                name: "empty".into(),
                description: "no props".into(),
                config_schema: serde_json::json!({"type": "object"}),
                rpc_params_schema: serde_json::json!({"type": "object"}),
            },
        ];
        let known = known_config_keys(&schemas);
        assert!(known.contains("config.a.x"));
        assert!(known.contains("config.b.x"));
        assert!(!known.contains("config.missing"));
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
    fn prop_idempotent() {
        run_proptest(
            prop::collection::vec(arb_any_chunk(), 0..20),
            |chunks: Vec<Chunk>| {
                let cfg1 = resolve_session_config(&chunks, &[]);
                let cfg2 = resolve_session_config(&chunks, &[]);
                assert_eq!(cfg1.values, cfg2.values);
                assert_eq!(cfg1.extra, cfg2.extra);
            },
        );
    }

    #[test]
    fn prop_extra_is_subset_of_values() {
        run_proptest(
            prop::collection::vec(arb_any_chunk(), 0..20),
            |chunks: Vec<Chunk>| {
                let cfg = resolve_session_config(&chunks, &[]);
                for (k, v) in &cfg.extra {
                    assert_eq!(cfg.values.get(k), Some(v));
                }
            },
        );
    }

    #[test]
    fn prop_partial_update_preserves_unset_fields() {
        run_proptest(
            prop::collection::vec(arb_runtime_config_chunk(), 0..10),
            |chunks: Vec<Chunk>| {
                let cfg = resolve_session_config(&chunks, &[]);
                let mut twice = chunks.clone();
                twice.extend(chunks);
                let cfg2 = resolve_session_config(&twice, &[]);
                assert_eq!(cfg.values, cfg2.values);
            },
        );
    }

    #[test]
    fn prop_non_config_chunks_ignored() {
        run_proptest(
            prop::collection::vec(arb_any_chunk(), 0..20),
            |chunks: Vec<Chunk>| {
                let _cfg = resolve_session_config(&chunks, &[]);
                // should not crash
            },
        );
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
                let _cfg = resolve_session_config(&combined, &[]);
            },
        );
    }
}
