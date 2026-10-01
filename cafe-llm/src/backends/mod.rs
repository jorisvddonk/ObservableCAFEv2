use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::BoxStream;

pub use cafe_sdk::BackendModels;

pub mod ollama;
pub mod openai;
pub mod opencode_go;
pub mod router;

#[derive(Debug, Clone, PartialEq)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

pub struct LlmParams {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Cafe session this generation belongs to. Backends that support
    /// conversation-scoped routing/caching (e.g. OpenCode Go's
    /// `x-opencode-session` header) forward it; others ignore it.
    pub session_id: Option<String>,
    /// Per-session backend override (e.g. `"opencode-go"`). Only the router
    /// consults this; plain backends ignore it.
    pub backend: Option<String>,
}

#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Stream response tokens for a given conversation context.
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        params: &LlmParams,
    ) -> Result<BoxStream<'static, Result<String>>>;

    /// Non-streaming convenience: collect full response text from `complete`.
    async fn complete_to_string(
        &self,
        messages: Vec<LlmMessage>,
        params: &LlmParams,
    ) -> Result<String> {
        use futures_util::StreamExt;
        let mut stream = self.complete(messages, params).await?;
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            text.push_str(&chunk?);
        }
        Ok(text)
    }

    /// List available models from the backend.
    async fn list_models(&self) -> Result<Vec<String>>;

    /// List models grouped by backend, for a caller that needs to know which
    /// provider serves each model. Plain backends report nothing; the router
    /// reports every configured backend.
    async fn list_backends(&self) -> Result<Vec<BackendModels>> {
        Ok(Vec::new())
    }

    /// The backend a session uses when it sets no `config.llm.backend`, and
    /// that backend's default model. Plain backends return `None`.
    fn default_backend(&self) -> Option<(String, Option<String>)> {
        None
    }

    /// Default model to use when a session selects a provider but not a model.
    /// Only the router (which owns multiple providers) returns `Some`; plain
    /// backends return `None`.
    fn default_model_for(&self, _backend: Option<&str>) -> Option<String> {
        None
    }
}
