pub struct Config {
    pub socket_path: String,
    pub agent_paths: Vec<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let paths_str = std::env::var("CAFE_AGENT_PATHS")
            .unwrap_or_else(|_| "./agents".into());
        let agent_paths = paths_str
            .split(':')
            .map(String::from)
            .collect();
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            agent_paths,
        }
    }
}
