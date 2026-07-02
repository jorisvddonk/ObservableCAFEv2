mod sse;
pub use sse::try_parse_sse_chunk;

use crate::error::SdkError;
use cafe_types::{Chunk, SessionInfo};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;

/// HTTP API client for the cafe-server REST API.
pub struct HttpClient {
    client: Client,
    base_url: String,
    token: String,
}

impl HttpClient {
    /// Create a new HTTP client for the cafe-server at `base_url`
    /// authenticating with `token`.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            token: token.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// List all sessions.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, SdkError> {
        let resp = self
            .client
            .get(self.url("/api/sessions"))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<SessionInfo>>()
            .await?;
        Ok(resp)
    }

    /// Create a new session for the given agent. Returns the session ID.
    pub async fn create_session(&self, agent_id: &str) -> Result<String, SdkError> {
        let resp = self
            .client
            .post(self.url("/api/sessions"))
            .bearer_auth(&self.token)
            .json(&json!({ "agent_id": agent_id }))
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        Ok(resp["id"].as_str().unwrap_or("").to_string())
    }

    /// Delete a session.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SdkError> {
        self.client
            .delete(self.url(&format!("/api/sessions/{}", session_id)))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Get the full history of a session.
    pub async fn get_history(&self, session_id: &str) -> Result<Vec<Chunk>, SdkError> {
        let resp = self
            .client
            .get(self.url(&format!("/api/sessions/{}/history", session_id)))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        let chunks = resp["chunks"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect())
            .unwrap_or_default();
        Ok(chunks)
    }

    /// Stream a chat response. Sends a user message and yields received
    /// chunks over the channel. Returns when the stream ends or an error
    /// occurs.
    pub async fn stream_chat(
        &self,
        session_id: &str,
        message: &str,
        tx: mpsc::Sender<Chunk>,
    ) -> Result<(), SdkError> {
        let mut stream = self
            .client
            .post(self.url(&format!("/api/sessions/{}/chat", session_id)))
            .bearer_auth(&self.token)
            .json(&json!({ "content": message }))
            .send()
            .await?
            .bytes_stream();

        let mut buffer = String::new();

        while let Some(bytes) = stream.next().await {
            let text = String::from_utf8_lossy(&bytes?).to_string();
            buffer.push_str(&text);
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

    /// Set the system prompt for a session (injects a config chunk).
    pub async fn set_system_prompt(&self, session_id: &str, prompt: &str) -> Result<(), SdkError> {
        self.client
            .post(self.url(&format!("/api/sessions/{}/chunks", session_id)))
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

    /// Rename a session.
    pub async fn rename_session(&self, session_id: &str, name: &str) -> Result<(), SdkError> {
        self.client
            .post(self.url(&format!("/api/sessions/{}/chunks", session_id)))
            .bearer_auth(&self.token)
            .json(&json!({
                "content_type": "null",
                "annotations": {
                    "config.session.name": name
                }
            }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Switch the model for a session.
    pub async fn set_model(&self, session_id: &str, model: &str) -> Result<(), SdkError> {
        self.client
            .post(self.url(&format!("/api/sessions/{}/chunks", session_id)))
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

    /// List available models.
    pub async fn list_models(&self) -> Result<Vec<String>, SdkError> {
        let resp = self
            .client
            .get(self.url("/api/models"))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        let models = resp["models"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        Ok(models)
    }
}
