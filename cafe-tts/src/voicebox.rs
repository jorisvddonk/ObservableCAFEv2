/// HTTP client for the Voicebox TTS API.
pub struct VoiceboxClient {
    /// Base URL, e.g. "http://127.0.0.1:17493"
    pub base_url: String,
    http: reqwest::Client,
}

impl VoiceboxClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// POST /speak — returns (audio_bytes, mime_type).
    ///
    /// `profile` is the Voicebox profile name (not a UUID).
    /// `engine` is an optional query parameter passed to Voicebox.
    pub async fn speak(
        &self,
        text: &str,
        profile: &str,
        engine: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let url = match engine {
            Some(e) => format!("{}/speak?engine={}", self.base_url, e),
            None => format!("{}/speak", self.base_url),
        };

        let body = serde_json::json!({
            "text": text,
            "profile": profile,
        });

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let mime = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("audio/wav")
            .to_string();

        let bytes = response.bytes().await?.to_vec();
        Ok((bytes, mime))
    }
}
