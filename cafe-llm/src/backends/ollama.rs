use super::{LlmBackend, LlmMessage, LlmParams};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

pub struct OllamaBackend {
    client: Client,
    base_url: String,
}

impl OllamaBackend {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }
}

#[derive(Deserialize)]
struct OllamaChunk {
    message: Option<OllamaMessage>,
    #[allow(dead_code)]
    done: bool,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

#[async_trait]
impl LlmBackend for OllamaBackend {
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
            body["options"] = json!({ "temperature": t });
        }

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Ollama error {}: {}", status, text));
        }

        let byte_stream = response.bytes_stream();

        let token_stream = byte_stream
            .map(|result| -> Result<String> {
                let bytes = result?;
                let text = String::from_utf8_lossy(&bytes);
                // Each line is a JSON object
                let mut tokens = String::new();
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(chunk) = serde_json::from_str::<OllamaChunk>(line) {
                        if let Some(msg) = chunk.message {
                            tokens.push_str(&msg.content);
                        }
                    }
                }
                Ok(tokens)
            })
            .filter(|r| {
                // Filter out empty token strings (but keep errors)
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                futures_util::future::ready(keep)
            });

        Ok(Box::pin(token_stream))
    }
}
