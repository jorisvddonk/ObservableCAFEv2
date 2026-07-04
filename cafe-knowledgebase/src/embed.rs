use anyhow::Result;
use serde_json::json;

/// Embedding configuration.
#[derive(Clone)]
pub struct EmbedConfig {
    pub url: String,
    pub model: String,
    pub dim: usize,
}

impl EmbedConfig {
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("CAFE_KNOWLEDGEBASE_EMBED_URL")
                .unwrap_or_else(|_| "http://localhost:11434/api/embed".into()),
            model: std::env::var("CAFE_KNOWLEDGEBASE_EMBED_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".into()),
            dim: std::env::var("CAFE_KNOWLEDGEBASE_EMBED_DIM")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(768),
        }
    }
}

/// Generate an embedding vector for the given text by calling the embedding API.
pub async fn embed_text(config: &EmbedConfig, text: &str) -> Result<Vec<f32>> {
    let client = reqwest::Client::new();
    let payload = json!({
        "model": config.model,
        "input": text,
    });
    let resp = client.post(&config.url).json(&payload).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("embed API error {}: {}", status, body);
    }

    let body: serde_json::Value = resp.json().await?;

    // Try Ollama format: { "embeddings": [[f32; dim]] }
    if let Some(embeddings) = body["embeddings"].as_array() {
        if let Some(first) = embeddings.first() {
            return Ok(first
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect());
        }
    }

    // Try OpenAI format: { "data": [{ "embedding": [f32; dim] }] }
    if let Some(data) = body["data"].as_array() {
        if let Some(first) = data.first() {
            if let Some(embedding) = first["embedding"].as_array() {
                return Ok(embedding
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect());
            }
        }
    }

    // Try single embedding: { "embedding": [f32; dim] }
    if let Some(embedding) = body["embedding"].as_array() {
        return Ok(embedding
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect());
    }

    anyhow::bail!("unable to parse embedding from response: {}", body)
}
