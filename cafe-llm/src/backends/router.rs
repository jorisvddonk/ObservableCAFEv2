use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use tracing::warn;

use super::{BackendModels, LlmBackend, LlmMessage, LlmParams};

/// Canonicalize a backend name so `opencode_go` and `opencode-go` are the same
/// provider. Names are matched case-insensitively.
fn normalize(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "opencode_go" | "opencodego" => "opencode-go".to_string(),
        other => other.to_string(),
    }
}

struct BackendEntry {
    name: String,
    backend: Arc<dyn LlmBackend>,
    default_model: Option<String>,
}

/// Holds every configured backend and dispatches each request to the provider
/// selected by `LlmParams.backend`, falling back to a process-level default.
/// This is what makes `config.llm.backend` selectable per session without
/// restarting the service (mirrors the TTS approach in ADR-122).
pub struct BackendRouter {
    entries: Vec<BackendEntry>,
    default_name: String,
}

impl BackendRouter {
    /// `entries` are `(name, backend, default_model)`. `default_name` selects
    /// the provider used when a session does not override it.
    pub fn new(
        default_name: impl Into<String>,
        entries: Vec<(String, Arc<dyn LlmBackend>, Option<String>)>,
    ) -> Self {
        assert!(
            !entries.is_empty(),
            "BackendRouter requires at least one backend"
        );
        let default_name = default_name.into();
        let entries = entries
            .into_iter()
            .map(|(name, backend, default_model)| BackendEntry {
                name: normalize(&name),
                backend,
                default_model,
            })
            .collect();
        Self {
            entries,
            default_name: normalize(&default_name),
        }
    }

    /// Resolve a requested backend name to an entry, warning and falling back
    /// to the process default when it is unknown.
    fn resolve(&self, requested: Option<&str>) -> &BackendEntry {
        if let Some(req) = requested.filter(|s| !s.is_empty()) {
            let wanted = normalize(req);
            match self.entries.iter().find(|e| e.name == wanted) {
                Some(entry) => return entry,
                None => warn!(
                    "cafe-llm: unknown backend {:?}, falling back to {:?}",
                    req, self.default_name
                ),
            }
        }
        self.entries
            .iter()
            .find(|e| e.name == self.default_name)
            .unwrap_or(&self.entries[0])
    }
}

#[async_trait]
impl LlmBackend for BackendRouter {
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        params: &LlmParams,
    ) -> Result<BoxStream<'static, Result<String>>> {
        self.resolve(params.backend.as_deref())
            .backend
            .complete(messages, params)
            .await
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let mut models = Vec::new();
        for entry in &self.entries {
            match entry.backend.list_models().await {
                Ok(list) => models.extend(list),
                Err(e) => warn!(
                    "cafe-llm: backend {} model listing failed: {}",
                    entry.name, e
                ),
            }
        }
        models.sort();
        models.dedup();
        Ok(models)
    }

    async fn list_backends(&self) -> Result<Vec<BackendModels>> {
        let mut out = Vec::new();
        for entry in &self.entries {
            let models = match entry.backend.list_models().await {
                Ok(mut list) => {
                    list.sort();
                    list.dedup();
                    list
                }
                Err(e) => {
                    warn!(
                        "cafe-llm: backend {} model listing failed: {}",
                        entry.name, e
                    );
                    Vec::new()
                }
            };
            out.push(BackendModels {
                backend: entry.name.clone(),
                default_model: entry.default_model.clone(),
                models,
            });
        }
        Ok(out)
    }

    fn default_model_for(&self, backend: Option<&str>) -> Option<String> {
        self.resolve(backend).default_model.clone()
    }

    fn default_backend(&self) -> Option<(String, Option<String>)> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.name == self.default_name)
            .unwrap_or(&self.entries[0]);
        Some((entry.name.clone(), entry.default_model.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    struct FakeBackend {
        tag: String,
        models: Vec<String>,
    }

    #[async_trait]
    impl LlmBackend for FakeBackend {
        async fn complete(
            &self,
            _messages: Vec<LlmMessage>,
            _params: &LlmParams,
        ) -> Result<BoxStream<'static, Result<String>>> {
            let tag = self.tag.clone();
            Ok(Box::pin(futures_util::stream::once(async move { Ok(tag) })))
        }

        async fn list_models(&self) -> Result<Vec<String>> {
            Ok(self.models.clone())
        }
    }

    fn fake(tag: &str, models: &[&str]) -> Arc<dyn LlmBackend> {
        Arc::new(FakeBackend {
            tag: tag.to_string(),
            models: models.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn router() -> BackendRouter {
        BackendRouter::new(
            "ollama",
            vec![
                (
                    "ollama".into(),
                    fake("ollama", &["local-a"]),
                    Some("gemma3:1b".into()),
                ),
                (
                    "openai".into(),
                    fake("openai", &["local-b"]),
                    Some("ornith".into()),
                ),
                (
                    "opencode-go".into(),
                    fake("go", &["go-large"]),
                    Some("go-large".into()),
                ),
            ],
        )
    }

    fn params(backend: Option<&str>) -> LlmParams {
        LlmParams {
            model: "m".into(),
            temperature: None,
            max_tokens: None,
            session_id: None,
            backend: backend.map(String::from),
        }
    }

    async fn completed_text(router: &BackendRouter, backend: Option<&str>) -> String {
        let mut stream = router.complete(Vec::new(), &params(backend)).await.unwrap();
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            out.push_str(&chunk.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn routes_to_default_when_unset() {
        assert_eq!(completed_text(&router(), None).await, "ollama");
    }

    #[tokio::test]
    async fn routes_to_requested_backend() {
        assert_eq!(completed_text(&router(), Some("opencode-go")).await, "go");
        assert_eq!(completed_text(&router(), Some("openai")).await, "openai");
    }

    #[tokio::test]
    async fn normalizes_backend_aliases_and_case() {
        assert_eq!(completed_text(&router(), Some("opencode_go")).await, "go");
        assert_eq!(completed_text(&router(), Some("OpenCode-Go")).await, "go");
    }

    #[tokio::test]
    async fn unknown_backend_falls_back_to_default() {
        assert_eq!(
            completed_text(&router(), Some("does-not-exist")).await,
            "ollama"
        );
    }

    #[tokio::test]
    async fn default_model_is_per_backend() {
        let r = router();
        assert_eq!(r.default_model_for(None).as_deref(), Some("gemma3:1b"));
        assert_eq!(
            r.default_model_for(Some("openai")).as_deref(),
            Some("ornith")
        );
        assert_eq!(
            r.default_model_for(Some("opencode_go")).as_deref(),
            Some("go-large")
        );
        assert_eq!(
            r.default_model_for(Some("unknown")).as_deref(),
            Some("gemma3:1b")
        );
    }

    #[tokio::test]
    async fn list_models_aggregates_backends() {
        let models = router().list_models().await.unwrap();
        assert_eq!(models, vec!["go-large", "local-a", "local-b"]);
    }
}
