use crate::voicebox::VoiceboxClient;
use anyhow::Context;

/// HTTP client for speech-server's POST /speak endpoint (routed through proxy).
pub struct SpeechServerClient {
    pub base_url: String,
    http: reqwest::Client,
}

impl SpeechServerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn synthesize(
        &self,
        text: &str,
        engine: Option<&str>,
        _language: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        if text.is_empty() {
            anyhow::bail!("synthesize: text is empty");
        }

        let url = format!("{}/speak", self.base_url.trim_end_matches('/'));

        let mut body = serde_json::json!({ "text": text });
        if let Some(e) = engine {
            body["engine"] = serde_json::Value::String(e.to_string());
        } else {
            body["model"] = serde_json::Value::String("qwen3-tts".into());
        }
        if let Some(l) = _language {
            body["language"] = serde_json::Value::String(l.to_string());
        }

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /speak request failed")?
            .error_for_status()
            .context("POST /speak returned error status")?;

        let mime = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("audio/wav")
            .to_string();

        let audio_bytes = response
            .bytes()
            .await
            .context("Failed to read response body")?
            .to_vec();

        if audio_bytes.is_empty() {
            anyhow::bail!("POST /speak returned an empty body");
        }

        Ok((audio_bytes, mime))
    }
}

/// Unified client enum — worker code dispatches without caring which backend.
pub enum TtsClient {
    Voicebox(VoiceboxClient),
    SpeechServer(SpeechServerClient),
}

impl TtsClient {
    pub async fn synthesize(
        &self,
        text: &str,
        profile: &str,
        engine: Option<&str>,
        language: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        match self {
            TtsClient::Voicebox(c) => c.synthesize(text, profile, engine).await,
            TtsClient::SpeechServer(c) => c.synthesize(text, engine, language).await,
        }
    }
}
