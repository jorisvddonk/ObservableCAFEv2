mod sse;
pub use sse::{try_parse_sse_chunk, SseParseOutcome};

use crate::error::SdkError;
use cafe_types::{Chunk, SessionInfo};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

/// Info about an agent, returned by list_agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub description: String,
    pub background: bool,
}

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
            loop {
                match try_parse_sse_chunk(&mut buffer) {
                    SseParseOutcome::Chunk(chunk) => {
                        let is_complete = chunk
                            .get_annotation::<bool>("chat.stream_complete")
                            .unwrap_or(false);
                        tx.send(chunk).await.ok();
                        if is_complete {
                            return Ok(());
                        }
                    }
                    SseParseOutcome::Invalid { raw, error } => {
                        tracing::warn!(
                            target: "cafe_sdk::http",
                            "dropping invalid SSE frame: {error} (raw: {raw})"
                        );
                    }
                    SseParseOutcome::Incomplete => break,
                }
            }
        }
        Ok(())
    }

    /// Subscribe to live session events (SSE stream).
    /// Sends all live chunks through the channel until the stream ends.
    pub async fn subscribe_session(
        &self,
        session_id: &str,
        tx: mpsc::Sender<Chunk>,
    ) -> Result<(), SdkError> {
        let mut stream = self
            .client
            .get(self.url(&format!("/api/sessions/{}/stream", session_id)))
            .bearer_auth(&self.token)
            .send()
            .await?
            .bytes_stream();

        let mut buffer = String::new();

        while let Some(bytes) = stream.next().await {
            let text = String::from_utf8_lossy(&bytes?).to_string();
            buffer.push_str(&text);
            loop {
                match try_parse_sse_chunk(&mut buffer) {
                    SseParseOutcome::Chunk(chunk) => {
                        tx.send(chunk).await.ok();
                    }
                    SseParseOutcome::Invalid { raw, error } => {
                        tracing::warn!(
                            target: "cafe_sdk::http",
                            "dropping invalid SSE frame: {error} (raw: {raw})"
                        );
                    }
                    SseParseOutcome::Incomplete => break,
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

    /// Set the tags for a session.
    pub async fn set_tags(&self, session_id: &str, tags: &[String]) -> Result<(), SdkError> {
        self.client
            .patch(self.url(&format!("/api/sessions/{}/tags", session_id)))
            .bearer_auth(&self.token)
            .json(&json!({ "tags": tags }))
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

    /// Publish a chunk to a session (POST /api/sessions/:id/chunks).
    /// `base64_data` is the base64-encoded binary payload (for binary chunks).
    pub async fn publish_chunk(
        &self,
        session_id: &str,
        content_type: &str,
        content: Option<&str>,
        data: Option<&str>,
        mime_type: Option<&str>,
        annotations: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<(), SdkError> {
        let mut body = json!({
            "content_type": content_type,
        });
        if let Some(c) = content {
            body["content"] = json!(c);
        }
        if let Some(d) = data {
            body["data"] = json!(d);
        }
        if let Some(m) = mime_type {
            body["mime_type"] = json!(m);
        }
        if let Some(a) = annotations {
            body["annotations"] = json!(a);
        }
        self.client
            .post(self.url(&format!("/api/sessions/{}/chunks", session_id)))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// List available agents.
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>, SdkError> {
        let resp = self
            .client
            .get(self.url("/api/agents"))
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<AgentInfo>>()
            .await?;
        Ok(resp)
    }
}
