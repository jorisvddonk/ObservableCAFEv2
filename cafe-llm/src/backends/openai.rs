use super::{LlmBackend, LlmMessage, LlmParams};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::json;

pub struct OpenAiBackend {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiBackend {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
        }
    }
}

#[async_trait]
impl LlmBackend for OpenAiBackend {
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        params: &LlmParams,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let msgs: Vec<serde_json::Value> = messages
            .into_iter()
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect();

        let mut body = json!({
            "model": params.model,
            "messages": msgs,
            "stream": true,
        });

        if let Some(t) = params.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(mt) = params.max_tokens {
            body["max_tokens"] = json!(mt);
        }

        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body);

        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI error {}: {}", status, text));
        }

        let byte_stream = response.bytes_stream();

        let token_stream = byte_stream
            .map(|result| -> Result<String> {
                let bytes = result?;
                let text = String::from_utf8_lossy(&bytes);
                let mut tokens = String::new();
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line == "data: [DONE]" {
                        continue;
                    }
                    let data = line.strip_prefix("data: ").unwrap_or(line);
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(content) = val["choices"][0]["delta"]["content"].as_str() {
                            tokens.push_str(content);
                        }
                    }
                }
                Ok(tokens)
            })
            .filter(|r| {
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                futures_util::future::ready(keep)
            });

        Ok(Box::pin(token_stream))
    }
}
