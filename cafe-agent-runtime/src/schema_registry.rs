use cafe_sdk::{keys, Chunk, EvaluatorSchema, ServerMessage};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// In-memory registry of evaluator schemas, populated from the `__schema__`
/// session on the bus.
#[derive(Clone)]
pub struct SchemaRegistry {
    inner: Arc<RwLock<HashMap<String, EvaluatorSchema>>>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get(&self, name: &str) -> Option<EvaluatorSchema> {
        self.inner.read().await.get(name).cloned()
    }

    pub async fn insert(&self, schema: EvaluatorSchema) {
        let name = schema.name.clone();
        self.inner.write().await.insert(name, schema);
    }

    /// Subscribe to the `__schema__` session, collect existing schemas from
    /// history, and spawn a background task that keeps the registry updated as
    /// new evaluators announce themselves.
    pub async fn start_discovery(
        self,
        socket_path: String,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.try_discover(&socket_path).await {
                    Ok(()) => {
                        info!("schema_registry: discovery task ended (session dropped)");
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "schema_registry: discovery error, retrying: {e}"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        })
    }

    async fn try_discover(&self, socket_path: &str) -> Result<(), anyhow::Error> {
        let bus = cafe_sdk::bus::BusClient::unix(socket_path);

        // Create schema session (idempotent)
        let _ = bus
            .create_session(
                cafe_types::schema::SCHEMA_SESSION,
                cafe_types::schema::SCHEMA_SESSION,
                cafe_sdk::SessionConfig::default(),
            )
            .await;

        // Subscribe — this gives us history replay + live chunks
        let mut rx = bus.subscribe(cafe_types::schema::SCHEMA_SESSION).await?;

        // First pass: collect existing schemas from history
        let mut history_done = false;
        while let Some(msg) = rx.recv().await {
            match msg {
                ServerMessage::Chunk { chunk, .. } => {
                    if let Some(schema) = extract_schema(&chunk) {
                        self.insert(schema).await;
                    }
                }
                ServerMessage::HistoryComplete { .. } => {
                    history_done = true;
                    break;
                }
                _ => {}
            }
        }

        if !history_done {
            return Err(anyhow::anyhow!("disconnected before history complete"));
        }

        // Second pass: listen for new schemas from evaluators starting later
        while let Some(msg) = rx.recv().await {
            if let ServerMessage::Chunk { chunk, .. } = msg {
                if let Some(schema) = extract_schema(&chunk) {
                    info!(
                        "schema_registry: discovered evaluator '{}': {}",
                        schema.name, schema.description
                    );
                    self.insert(schema).await;
                }
            }
        }

        Ok(())
    }
}

fn extract_schema(chunk: &Chunk) -> Option<EvaluatorSchema> {
    chunk
        .annotations
        .get(keys::CAFE_SCHEMA_EVALUATOR)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_sdk::Chunk;

    fn make_schema_chunk(name: &str) -> Chunk {
        let schema = EvaluatorSchema {
            name: name.into(),
            description: format!("{} evaluator", name),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
        };
        Chunk::new_null("test")
            .with_annotation(keys::CAFE_SCHEMA_EVALUATOR, &schema)
    }

    #[test]
    fn extract_schema_found() {
        let chunk = make_schema_chunk("llm");
        let schema = extract_schema(&chunk);
        assert!(schema.is_some());
        assert_eq!(schema.unwrap().name, "llm");
    }

    #[test]
    fn extract_schema_returns_none_for_missing_annotation() {
        let chunk = Chunk::new_null("test");
        assert!(extract_schema(&chunk).is_none());
    }

    #[test]
    fn extract_schema_returns_none_for_text_chunk() {
        let chunk = Chunk::new_text("hello", "test");
        assert!(extract_schema(&chunk).is_none());
    }

    #[test]
    fn extract_schema_invalid_value_returns_none() {
        let chunk = Chunk::new_null("test")
            .with_annotation(keys::CAFE_SCHEMA_EVALUATOR, "not-a-schema-object");
        let schema = extract_schema(&chunk);
        assert!(schema.is_none(), "invalid JSON should not deserialize");
    }

    #[tokio::test]
    async fn registry_insert_and_get() {
        let registry = SchemaRegistry::new();
        let schema = EvaluatorSchema {
            name: "llm".into(),
            description: "test".into(),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
        };
        registry.insert(schema.clone()).await;
        let retrieved = registry.get("llm").await;
        assert_eq!(retrieved.unwrap().name, "llm");
    }

    #[tokio::test]
    async fn registry_get_missing() {
        let registry = SchemaRegistry::new();
        let retrieved = registry.get("nonexistent").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn registry_insert_overwrites() {
        let registry = SchemaRegistry::new();
        let s1 = EvaluatorSchema {
            name: "llm".into(),
            description: "v1".into(),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
        };
        let s2 = EvaluatorSchema {
            name: "llm".into(),
            description: "v2".into(),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
        };
        registry.insert(s1).await;
        registry.insert(s2).await;
        let retrieved = registry.get("llm").await;
        assert_eq!(retrieved.unwrap().description, "v2");
    }

    #[tokio::test]
    async fn registry_multiple_schemas() {
        let registry = SchemaRegistry::new();
        for name in &["llm", "tts", "comfy", "rot13"] {
            registry
                .insert(EvaluatorSchema {
                    name: name.to_string(),
                    description: format!("{} evaluator", name),
                    config_schema: serde_json::json!({"type": "object", "properties": {}}),
                    rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
                })
                .await;
        }
        assert_eq!(registry.get("llm").await.unwrap().name, "llm");
        assert_eq!(registry.get("tts").await.unwrap().name, "tts");
        assert_eq!(registry.get("rot13").await.unwrap().name, "rot13");
        assert!(registry.get("unknown").await.is_none());
    }
}
