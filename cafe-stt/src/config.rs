#[derive(Clone, Copy, PartialEq)]
pub enum SttBackend {
    Voicebox,
    SpeechServer,
}

#[derive(Clone)]
pub struct Config {
    pub socket_path: String,
    pub voicebox_url: String,
    pub speech_server_url: String,
    pub backend: SttBackend,
}

impl Config {
    pub fn from_env() -> Self {
        let socket_path = std::env::var("CAFE_BUS_SOCKET")
            .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());
        let voicebox_url = std::env::var("VOICEBOX_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:17493".into());
        let speech_server_url = std::env::var("SPEECH_SERVER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:6900".into());
        let backend = match std::env::var("STT_BACKEND").as_deref() {
            Ok("speech-server") => SttBackend::SpeechServer,
            _ => SttBackend::Voicebox,
        };

        Self {
            socket_path,
            voicebox_url,
            speech_server_url,
            backend,
        }
    }
}
