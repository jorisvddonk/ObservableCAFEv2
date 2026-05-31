use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "cafe-tui", about = "ObservableCAFE terminal UI")]
pub struct Config {
    /// cafe-server URL
    #[arg(long, env = "CAFE_SERVER_URL", default_value = "http://localhost:3000")]
    pub url: String,

    /// API token
    #[arg(long, env = "CAFE_TOKEN", default_value = "")]
    pub token: String,
}

impl Config {
    pub fn from_args() -> Self {
        Self::parse()
    }
}
