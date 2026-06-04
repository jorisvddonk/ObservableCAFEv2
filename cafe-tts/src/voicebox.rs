use anyhow::{bail, Context};
use futures_util::StreamExt;
use serde::Deserialize;

// ── API response types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Profile {
    id: String,
    name: String,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the ObservableCAFE Voicebox API.
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

    /// Resolve a profile name to its UUID by calling GET /profiles.
    ///
    /// Returns an error if the API call fails or the profile name is not found.
    async fn resolve_profile_id(&self, profile_name: &str) -> anyhow::Result<String> {
        let url = format!("{}/profiles", self.base_url);
        let profiles: Vec<Profile> = self
            .http
            .get(&url)
            .send()
            .await
            .context("GET /profiles request failed")?
            .error_for_status()
            .context("GET /profiles returned error status")?
            .json()
            .await
            .context("failed to parse /profiles response as JSON")?;

        profiles
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(profile_name))
            .map(|p| p.id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Voicebox profile '{}' not found. \
                     Check VOICEBOX_URL and that the profile exists.",
                    profile_name
                )
            })
    }

    /// Generate speech via POST /generate/stream.
    ///
    /// 1. Resolves `profile_name` → `profile_id` via GET /profiles.
    /// 2. Streams the audio response body into memory.
    /// 3. Returns `(audio_bytes, mime_type)`.
    ///
    /// `engine` defaults to `"qwen"` if not supplied.
    pub async fn synthesize(
        &self,
        text: &str,
        profile_name: &str,
        engine: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        if text.is_empty() {
            bail!("synthesize: text is empty");
        }

        let profile_id = self
            .resolve_profile_id(profile_name)
            .await
            .with_context(|| format!("resolving profile '{}'", profile_name))?;

        let url = format!("{}/generate/stream", self.base_url);

        let body = serde_json::json!({
            "profile_id": profile_id,
            "text": text,
            "normalize": true,
            "max_chunk_chars": 800,
            "crossfade_ms": 50,
            "engine": engine.unwrap_or("qwen"),
        });

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /generate/stream request failed")?
            .error_for_status()
            .context("POST /generate/stream returned error status")?;

        // Detect mime type from the response before consuming the body
        let mime = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("audio/wav")
            .to_string();

        // Stream the body incrementally into a Vec<u8>
        let mut audio_bytes: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("error reading /generate/stream response body")?;
            audio_bytes.extend_from_slice(&chunk);
        }

        if audio_bytes.is_empty() {
            bail!("POST /generate/stream returned an empty body");
        }

        Ok((audio_bytes, mime))
    }
}
