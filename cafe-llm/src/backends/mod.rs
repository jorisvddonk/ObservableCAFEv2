use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::BoxStream;

pub mod ollama;
pub mod openai;

pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

pub struct LlmParams {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
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
}
