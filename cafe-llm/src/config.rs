pub struct Config {
    pub socket_path: String,
    pub backend: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub openai_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            backend: std::env::var("LLM_BACKEND")
                .unwrap_or_else(|_| "ollama".into()),
            ollama_url: std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            ollama_model: std::env::var("OLLAMA_MODEL")
                .unwrap_or_else(|_| "gemma3:1b".into()),
            openai_url: std::env::var("OPENAI_URL")
                .unwrap_or_else(|_| "http://localhost:8000".into()),
            openai_api_key: std::env::var("OPENAI_API_KEY")
                .unwrap_or_default(),
            openai_model: std::env::var("OPENAI_MODEL")
                .unwrap_or_default(),
        }
    }
}
