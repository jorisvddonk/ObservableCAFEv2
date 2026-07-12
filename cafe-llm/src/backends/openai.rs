use super::{LlmBackend, LlmMessage, LlmParams};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

pub struct OpenAiBackend {
    client: Client,
    base_url: String,
    api_key: String,
    model_list_urls: Vec<String>,
}

impl OpenAiBackend {
    pub fn new(base_url: String, api_key: String, model_list_urls: Vec<String>) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
            model_list_urls,
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
            .scan(Vec::<u8>::new(), |buf, result| {
                let mut tokens = Vec::new();
                let bytes = match result {
                    Ok(b) => b,
                    Err(e) => return futures_util::future::ready(Some(Err(anyhow::anyhow!("{e}")))),
                };
                buf.extend_from_slice(&bytes);

                while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                    let frame = buf.drain(..pos + 2).collect::<Vec<_>>();
                    let text = String::from_utf8_lossy(&frame);
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let data = line.strip_prefix("data: ").unwrap_or(line);
                        if data == "[DONE]" {
                            continue;
                        }
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(content) = val["choices"][0]["delta"]["content"].as_str() {
                                tokens.push(content.to_string());
                            }
                        }
                    }
                }

                futures_util::future::ready(Some(Ok(tokens.join(""))))
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

    async fn list_models(&self) -> Result<Vec<String>> {
        let mut models = Vec::new();

        // Query the primary URL
        if let Ok(list) = self.fetch_models(&self.base_url).await {
            models.extend(list);
        }

        // Query additional model list URLs
        for url in &self.model_list_urls {
            if let Ok(list) = self.fetch_models(url).await {
                models.extend(list);
            }
        }

        models.sort();
        models.dedup();
        Ok(models)
    }
}

impl OpenAiBackend {
    async fn fetch_models(&self, url: &str) -> Result<Vec<String>> {
        let mut req = self.client.get(format!("{}/v1/models", url.trim_end_matches('/')));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req.send().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let list: OpenAiModelList = resp.json().await?;
        Ok(list.data.into_iter().map(|m| m.id).collect())
    }
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

#[derive(Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}
