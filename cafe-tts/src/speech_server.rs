use crate::config::TtsBackend;
use crate::voicebox::VoiceboxClient;
use anyhow::Context;
use tracing::warn;

/// HTTP client for speech-server's POST /speak endpoint (routed through proxy).
#[derive(Clone)]
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
            body["model"] = serde_json::Value::String("cosyvoice-3".into());
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

/// Owns both TTS backends and dispatches per request.
///
/// The backend is selected per session from `config.tts.backend`
/// (with an optional `config.tts.endpoint` URL override), falling back to
/// the process-level default when a session does not specify one.
pub struct TtsService {
    voicebox: VoiceboxClient,
    speech_server: SpeechServerClient,
    default_backend: TtsBackend,
}

impl TtsService {
    pub fn new(
        voicebox: VoiceboxClient,
        speech_server: SpeechServerClient,
        default_backend: TtsBackend,
    ) -> Self {
        Self {
            voicebox,
            speech_server,
            default_backend,
        }
    }

    pub fn default_backend(&self) -> TtsBackend {
        self.default_backend
    }

    pub async fn synthesize(
        &self,
        backend: Option<&str>,
        endpoint: Option<&str>,
        text: &str,
        profile: &str,
        engine: Option<&str>,
        language: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let backend = match backend {
            Some(raw) if !raw.trim().is_empty() => match TtsBackend::parse(raw) {
                Some(b) => b,
                None => {
                    warn!(
                        "cafe-tts: unknown backend {:?}, using default {:?}",
                        raw, self.default_backend
                    );
                    self.default_backend
                }
            },
            _ => self.default_backend,
        };

        match backend {
            TtsBackend::Voicebox => {
                let mut client = self.voicebox.clone();
                if let Some(e) = endpoint {
                    if !e.trim().is_empty() {
                        client.base_url = e.trim().to_string();
                    }
                }
                client.synthesize(text, profile, engine).await
            }
            TtsBackend::SpeechServer => {
                let mut client = self.speech_server.clone();
                if let Some(e) = endpoint {
                    if !e.trim().is_empty() {
                        client.base_url = e.trim().to_string();
                    }
                }
                client.synthesize(text, engine, language).await
            }
        }
    }
}
