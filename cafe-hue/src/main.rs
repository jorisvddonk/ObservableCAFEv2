mod config;
mod hue_client;
mod worker;

use config::Config;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Arc::new(Config::from_env());

    info!(
        "cafe-hue: starting — bus={} bridge={}",
        config.socket_path, config.bridge_url
    );

    wait_for_bus(&config.socket_path).await;

    worker::run_with_reconnect(config.socket_path.clone(), config).await;

    Ok(())
}

async fn wait_for_bus(socket_path: &str) {
    let path = std::path::Path::new(socket_path);
    let mut attempts = 0u32;
    while !path.exists() {
        if attempts == 0 {
            info!("cafe-hue: waiting for bus at {}", socket_path);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
        if attempts > 60 {
            tracing::warn!("cafe-hue: bus not ready after 30s, continuing anyway");
            break;
        }
    }
}
