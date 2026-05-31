pub struct Config {
    pub socket_path: String,
    pub db_path: String,
    pub port: u16,
    pub admin_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            db_path: std::env::var("CAFE_DB_PATH")
                .unwrap_or_else(|_| "./cafe.db".into()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            admin_token: std::env::var("CAFE_ADMIN_TOKEN").ok(),
        }
    }
}
