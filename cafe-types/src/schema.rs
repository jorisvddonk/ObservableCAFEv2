use serde::{Deserialize, Serialize};

/// Well-known session name where evaluators publish their schemas.
pub const SCHEMA_SESSION: &str = "__schema__";

/// An evaluator's self-describing schema, advertised on the `__schema__`
/// session as a null chunk with `cafe.schema.evaluator` annotation.
///
/// Both `config_schema` and `rpc_params_schema` are JSON Schema
/// (draft-07) objects describing the evaluator's configuration keys
/// (set via runtime-config null chunks) and its RPC-invoke parameters
/// (sent in `{name}.invoke` requests).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatorSchema {
    /// Evaluator name, used as step `type` in agent TOML files
    /// (e.g. `"llm"`, `"tts"`, `"comfy"`, `"rot13"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema (draft-07) for config annotation keys.
    /// Properties use full keys (e.g. `"config.llm.system_prompt"`).
    #[serde(default = "default_object")]
    pub config_schema: serde_json::Value,
    /// JSON Schema (draft-07) for the params sent in the
    /// `{name}.invoke` RPC call.
    #[serde(default = "default_object")]
    pub rpc_params_schema: serde_json::Value,
}

fn default_object() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_session_constant() {
        assert_eq!(SCHEMA_SESSION, "__schema__");
    }

    #[test]
    fn default_object_shape() {
        let obj = default_object();
        assert_eq!(obj["type"], "object");
        assert!(obj["properties"].is_object());
        assert!(obj["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn evaluator_schema_minimal() {
        let schema = EvaluatorSchema {
            name: "test".into(),
            description: "A test evaluator".into(),
            config_schema: default_object(),
            rpc_params_schema: default_object(),
        };
        assert_eq!(schema.name, "test");
        assert_eq!(schema.description, "A test evaluator");
    }

    #[test]
    fn evaluator_schema_serde_roundtrip() {
        let schema = EvaluatorSchema {
            name: "llm".into(),
            description: "LLM evaluator".into(),
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "config.llm.temperature": {
                        "type": "number",
                        "default": 0.7
                    }
                }
            }),
            rpc_params_schema: serde_json::json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" }
                }
            }),
        };

        let json = serde_json::to_string(&schema).unwrap();
        let decoded: EvaluatorSchema = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.name, "llm");
        assert_eq!(decoded.description, "LLM evaluator");
        assert_eq!(
            decoded.config_schema["properties"]["config.llm.temperature"]["default"],
            0.7
        );
        assert!(decoded.rpc_params_schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("session_id")));
    }

    #[test]
    fn evaluator_schema_deserialize_missing_schemas() {
        let json = r#"{"name": "minimal", "description": "No schemas"}"#;
        let schema: EvaluatorSchema = serde_json::from_str(json).unwrap();
        assert_eq!(schema.name, "minimal");
        assert_eq!(schema.config_schema["type"], "object");
        assert_eq!(schema.rpc_params_schema["type"], "object");
    }

    #[test]
    fn evaluator_schema_serde_with_defaults() {
        let json = serde_json::to_string(&EvaluatorSchema {
            name: "empty".into(),
            description: "".into(),
            config_schema: default_object(),
            rpc_params_schema: default_object(),
        })
        .unwrap();
        let decoded: EvaluatorSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "empty");
    }
}
