use anyhow::{bail, Context};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct PromptResponse {
    prompt_id: String,
    number: u64,
}

#[derive(Debug, Deserialize)]
struct HistoryResponse {
    #[serde(flatten)]
    prompts: HashMap<String, HistoryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct HistoryEntry {
    outputs: HashMap<String, NodeOutput>,
    status: HistoryStatus,
}

#[derive(Debug, Clone, Deserialize)]
struct HistoryStatus {
    completed: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct NodeOutput {
    images: Option<Vec<GeneratedImage>>,
}

#[derive(Debug, Deserialize, Clone)]
struct GeneratedImage {
    filename: String,
    subfolder: String,
    #[serde(rename = "type")]
    image_type: String,
}

pub struct ComfyUIClient {
    pub base_url: String,
    http: reqwest::Client,
}

impl ComfyUIClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn generate(
        &self,
        workflow: &serde_json::Value,
        prompt_text: &str,
        input_node: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let mut workflow = workflow.clone();
        inject_prompt(&mut workflow, input_node, prompt_text);

        let url = format!("{}/prompt", self.base_url);
        let body = serde_json::json!({
            "prompt": workflow,
            "client_id": "cafe-comfy",
        });

        let resp: PromptResponse = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /prompt request failed")?
            .error_for_status()
            .context("POST /prompt returned error status")?
            .json()
            .await
            .context("failed to parse /prompt response")?;

        let prompt_id = resp.prompt_id;

        let history = self.poll_history(&prompt_id).await?;

        let image = history
            .outputs
            .values()
            .find_map(|o| o.images.as_ref()?.first().cloned())
            .ok_or_else(|| anyhow::anyhow!("no images in ComfyUI output"))?;

        let image_bytes = self.download_image(&image).await?;

        Ok(image_bytes)
    }

    async fn poll_history(&self, prompt_id: &str) -> anyhow::Result<HistoryEntry> {
        let url = format!("{}/history/{}", self.base_url, prompt_id);
        for _ in 0..120 {
            let resp: HistoryResponse = self
                .http
                .get(&url)
                .send()
                .await
                .context("GET /history request failed")?
                .error_for_status()
                .context("GET /history returned error status")?
                .json()
                .await
                .context("failed to parse /history response")?;

            if let Some(entry) = resp.prompts.get(prompt_id) {
                if entry.status.completed {
                    return Ok(entry.clone());
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        bail!("ComfyUI prompt {} did not complete within 60s", prompt_id);
    }

    async fn download_image(&self, image: &GeneratedImage) -> anyhow::Result<Vec<u8>> {
        let url = format!(
            "{}/view?filename={}&subfolder={}&type={}",
            self.base_url, image.filename, image.subfolder, image.image_type
        );
        let bytes = self
            .http
            .get(&url)
            .send()
            .await
            .context("GET /view request failed")?
            .error_for_status()
            .context("GET /view returned error status")?
            .bytes()
            .await
            .context("failed to read image bytes")?;
        Ok(bytes.to_vec())
    }
}

fn inject_prompt(workflow: &mut serde_json::Value, node_id: &str, text: &str) {
    if let Some(node) = workflow.get_mut(node_id) {
        if let Some(inputs) = node.get_mut("inputs") {
            if let Some(text_field) = inputs.get_mut("text") {
                *text_field = serde_json::Value::String(text.to_string());
            }
        }
    }
}
