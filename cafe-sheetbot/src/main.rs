mod config;
mod sheetbot;
mod worker;

use config::Config;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use sheetbot::SheetbotClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let mut sheetbot = SheetbotClient::new(&config.sheetbot_url, &config.sheetbot_api_key);

    // Exchange API key for JWT if needed
    if let Err(e) = sheetbot.login().await {
        tracing::warn!("cafe-sheetbot: login failed (will try unauthenticated): {}", e);
    }

    let sheetbot = Arc::new(sheetbot);

    info!(
        "cafe-sheetbot: starting — bus={} sheetbot={}",
        config.socket_path, config.sheetbot_url
    );

    wait_for_bus(&config.socket_path).await;

    worker::run_with_reconnect(config.socket_path, sheetbot).await;

    Ok(())
}

async fn wait_for_bus(socket_path: &str) {
    let path = std::path::Path::new(socket_path);
    let mut attempts = 0u32;
    while !path.exists() {
        if attempts == 0 {
            info!("cafe-sheetbot: waiting for bus at {}", socket_path);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
        if attempts > 60 {
            tracing::warn!("cafe-sheetbot: bus not ready after 30s, continuing anyway");
            break;
        }
    }
}
