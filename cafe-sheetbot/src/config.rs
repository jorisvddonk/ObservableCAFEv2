pub struct Config {
    pub socket_path: String,
    pub sheetbot_url: String,
    pub sheetbot_api_key: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            sheetbot_url: std::env::var("SHEETBOT_URL")
                .unwrap_or_else(|_| "http://localhost:3000".into()),
            sheetbot_api_key: std::env::var("SHEETBOT_API_KEY")
                .unwrap_or_default(),
        }
    }
}
