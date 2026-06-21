use anyhow::Result;
use cafe_types::{Chunk, SessionInfo};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;

pub struct ApiClient {
    client: Client,
    base_url: String,
    token: String,
}

impl ApiClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            token,
        }
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let resp = self
            .client
            .get(format!("{}/api/sessions", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<SessionInfo>>()
            .await?;
        Ok(resp)
    }

    pub async fn create_session(&self, agent_id: &str) -> Result<String> {
        let resp = self
            .client
            .post(format!("{}/api/sessions", self.base_url))
            .bearer_auth(&self.token)
            .json(&json!({ "agent_id": agent_id }))
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        Ok(resp["id"].as_str().unwrap_or("").to_string())
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.client
            .delete(format!("{}/api/sessions/{}", self.base_url, session_id))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn get_history(&self, session_id: &str) -> Result<Vec<Chunk>> {
        let resp = self
            .client
            .get(format!(
                "{}/api/sessions/{}/history",
                self.base_url, session_id
            ))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        let chunks = resp["chunks"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Ok(chunks)
    }

    /// Stream a chat response. Sends user message, yields chunks via channel.
    pub async fn stream_chat(
        &self,
        session_id: &str,
        message: &str,
        tx: mpsc::Sender<Chunk>,
    ) -> Result<()> {
        let mut stream = self
            .client
            .post(format!(
                "{}/api/sessions/{}/chat",
                self.base_url, session_id
            ))
            .bearer_auth(&self.token)
            .json(&json!({ "content": message }))
            .send()
            .await?
            .bytes_stream();

        let mut buffer = String::new();

        while let Some(bytes) = stream.next().await {
            let text = String::from_utf8_lossy(&bytes?).to_string();
            buffer.push_str(&text);

            // Parse SSE lines: "data: {...}\n\n"
            while let Some(chunk) = try_parse_sse_chunk(&mut buffer) {
                let is_complete = chunk
                    .get_annotation::<bool>("chat.stream_complete")
                    .unwrap_or(false);
                tx.send(chunk).await.ok();
                if is_complete {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    pub async fn set_system_prompt(&self, session_id: &str, prompt: &str) -> Result<()> {
        self.client
            .post(format!(
                "{}/api/sessions/{}/chunks",
                self.base_url, session_id
            ))
            .bearer_auth(&self.token)
            .json(&json!({
                "content_type": "null",
                "annotations": {
                    "config.type": "runtime",
                    "config.system_prompt": prompt
                }
            }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn set_model(&self, session_id: &str, model: &str) -> Result<()> {
        self.client
            .post(format!(
                "{}/api/sessions/{}/chunks",
                self.base_url, session_id
            ))
            .bearer_auth(&self.token)
            .json(&json!({
                "content_type": "null",
                "annotations": {
                    "config.type": "runtime",
                    "config.model": model
                }
            }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/api/models", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        let models = resp["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }
}

fn try_parse_sse_chunk(buffer: &mut String) -> Option<Chunk> {
    // Look for "data: {...}\n" pattern
    let data_prefix = "data: ";
    if let Some(start) = buffer.find(data_prefix) {
        let rest = &buffer[start + data_prefix.len()..];
        if let Some(end) = rest.find('\n') {
            let json_str = &rest[..end];
            if let Ok(chunk) = serde_json::from_str::<Chunk>(json_str) {
                let consumed = start + data_prefix.len() + end + 1;
                buffer.drain(..consumed);
                return Some(chunk);
            } else {
                // Not a valid chunk, skip this line
                let consumed = start + data_prefix.len() + end + 1;
                buffer.drain(..consumed);
            }
        }
    }
    None
}
