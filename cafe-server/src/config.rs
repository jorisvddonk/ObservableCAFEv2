pub struct Config {
    pub socket_path: String,
    pub db_path: String,
    pub port: u16,
    pub admin_token: Option<String>,
    pub proxy_max_body_size: usize,
    pub proxy_gc_interval_secs: u64,
    pub proxy_stale_purge_secs: u64,
    pub bus_iroh_key: Option<String>,
    pub bus_iroh_relay: Option<String>,
    pub bus_iroh_alpn: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            db_path: std::env::var("CAFE_DB_PATH").unwrap_or_else(|_| "./cafe.db".into()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4000),
            admin_token: std::env::var("CAFE_ADMIN_TOKEN").ok(),
            proxy_max_body_size: std::env::var("CAFE_HTTP_PROXY_MAX_BODY_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1_048_576),
            proxy_gc_interval_secs: std::env::var("CAFE_HTTP_PROXY_GC_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            proxy_stale_purge_secs: std::env::var("CAFE_HTTP_PROXY_STALE_PURGE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
            bus_iroh_key: std::env::var("CAFE_BUS_IROH_KEY").ok(),
            bus_iroh_relay: std::env::var("CAFE_BUS_IROH_RELAY").ok(),
            bus_iroh_alpn: std::env::var("CAFE_BUS_IROH_ALPN").ok(),
        }
    }
}
