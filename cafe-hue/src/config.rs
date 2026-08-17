pub struct Config {
    pub socket_path: String,
    pub bridge_url: String,
    pub api_key: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            bridge_url: std::env::var("HUE_BRIDGE_URL").unwrap_or_default(),
            api_key: std::env::var("HUE_API_KEY").unwrap_or_default(),
        }
    }
}
