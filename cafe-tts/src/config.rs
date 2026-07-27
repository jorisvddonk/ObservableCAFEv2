/// Which TTS backend to use.
#[derive(Clone, Copy, PartialEq)]
pub enum TtsBackend {
    Voicebox,
    SpeechServer,
}

/// Runtime configuration loaded from environment variables.
pub struct Config {
    /// Unix socket path for the bus (CAFE_BUS_SOCKET).
    pub socket_path: String,
    /// Base URL for the Voicebox HTTP API (VOICEBOX_URL).
    pub voicebox_url: String,
    /// Base URL for speech-server (SPEECH_SERVER_URL).
    pub speech_server_url: String,
    /// Backend selection (TTS_BACKEND).
    pub backend: TtsBackend,
}

impl Config {
    pub fn from_env() -> Self {
        let socket_path = std::env::var("CAFE_BUS_SOCKET")
            .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());
        let voicebox_url = std::env::var("VOICEBOX_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:17493".into());
        let speech_server_url = std::env::var("SPEECH_SERVER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:6900".into());
        let backend = match std::env::var("TTS_BACKEND")
            .as_deref()
        {
            Ok("speech-server") => TtsBackend::SpeechServer,
            _ => TtsBackend::Voicebox,
        };

        Self {
            socket_path,
            voicebox_url,
            speech_server_url,
            backend,
        }
    }
}
