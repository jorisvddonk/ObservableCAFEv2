/// Which TTS backend to use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TtsBackend {
    Voicebox,
    SpeechServer,
}

impl TtsBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            TtsBackend::Voicebox => "voicebox",
            TtsBackend::SpeechServer => "speech-server",
        }
    }

    /// Parse a backend name from a session-config value. Unknown/empty → None.
    pub fn parse(s: &str) -> Option<TtsBackend> {
        match s.trim().to_ascii_lowercase().as_str() {
            "voicebox" => Some(TtsBackend::Voicebox),
            "speech-server" | "speechserver" | "speech" => Some(TtsBackend::SpeechServer),
            _ => None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_canonical_names() {
        assert_eq!(TtsBackend::parse("voicebox"), Some(TtsBackend::Voicebox));
        assert_eq!(
            TtsBackend::parse("speech-server"),
            Some(TtsBackend::SpeechServer)
        );
    }

    #[test]
    fn parse_is_case_insensitive_and_trims() {
        assert_eq!(TtsBackend::parse("  VOICEBOX "), Some(TtsBackend::Voicebox));
        assert_eq!(
            TtsBackend::parse("SpeechServer"),
            Some(TtsBackend::SpeechServer)
        );
    }

    #[test]
    fn parse_rejects_unknown_or_empty() {
        assert_eq!(TtsBackend::parse(""), None);
        assert_eq!(TtsBackend::parse("elevenlabs"), None);
        assert_eq!(TtsBackend::parse("  "), None);
    }

    #[test]
    fn as_str_round_trips() {
        for b in [TtsBackend::Voicebox, TtsBackend::SpeechServer] {
            assert_eq!(TtsBackend::parse(b.as_str()), Some(b));
        }
    }
}
