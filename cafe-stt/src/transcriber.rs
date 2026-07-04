use anyhow::Result;
use reqwest::multipart::{Form, Part};

/// Transcribe audio using voicebox's `/transcribe` endpoint.
pub async fn transcribe(
    voicebox_url: &str,
    audio: &[u8],
    mime_type: &str,
    language: Option<&str>,
    model: Option<&str>,
) -> Result<(String, f64)> {
    let url = format!("{}/transcribe", voicebox_url.trim_end_matches('/'));

    let audio_part = Part::bytes(audio.to_vec())
        .mime_str(mime_type)?
        .file_name("audio");

    let mut form = Form::new().part("file", audio_part);

    if let Some(lang) = language {
        form = form.text("language", lang.to_string());
    }
    if let Some(m) = model {
        form = form.text("model", m.to_string());
    }

    let client = reqwest::Client::new();
    let resp = client.post(&url).multipart(form).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("voicebox /transcribe error {}: {}", status, body);
    }

    let body: serde_json::Value = resp.json().await?;

    let text = body["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing text in response: {}", body))?
        .to_string();

    let duration = body["duration"].as_f64().unwrap_or(0.0);

    Ok((text, duration))
}
