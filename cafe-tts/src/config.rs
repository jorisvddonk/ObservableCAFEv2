/// Runtime configuration loaded from environment variables.
pub struct Config {
    /// Unix socket path for the bus (CAFE_BUS_SOCKET).
    pub socket_path: String,
    /// Base URL for the Voicebox HTTP API (VOICEBOX_URL).
    pub voicebox_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            voicebox_url: std::env::var("VOICEBOX_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:17493".into()),
        }
    }
}
