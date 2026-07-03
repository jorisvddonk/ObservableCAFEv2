use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "cafe-tui", about = "ObservableCAFE terminal UI")]
pub struct Config {
    /// cafe-server URL
    #[arg(long, env = "CAFE_SERVER_URL", default_value = "http://localhost:4000")]
    pub url: String,

    /// API token
    #[arg(long, env = "CAFE_TOKEN", default_value = "")]
    pub token: String,

    /// Create a new session on startup
    #[arg(long)]
    pub new: bool,

    /// Preset model for the new session
    #[arg(long)]
    pub model: Option<String>,

    /// Preset system prompt for the new session
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Agent to use for new sessions (default: "default")
    #[arg(long, default_value = "default")]
    pub agent: String,
}

impl Config {
    pub fn from_args() -> Self {
        Self::parse()
    }
}
