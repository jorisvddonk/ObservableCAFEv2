mod config;
mod comfyui;
mod worker;

use config::Config;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use comfyui::ComfyUIClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let comfy = Arc::new(ComfyUIClient::new(&config.comfy_url));

    let workflow_str = std::fs::read_to_string(&config.workflow_path)
        .map_err(|e| anyhow::anyhow!("failed to read workflow file '{}': {}", config.workflow_path, e))?;
    let workflow: serde_json::Value = serde_json::from_str(&workflow_str)
        .map_err(|e| anyhow::anyhow!("failed to parse workflow JSON: {}", e))?;

    info!(
        "cafe-comfy: starting — bus={} comfy={} workflow={} input_node={}",
        config.socket_path, config.comfy_url, config.workflow_path, config.workflow_input_node
    );

    wait_for_bus(&config.socket_path).await;

    worker::run_with_reconnect(config.socket_path, comfy, workflow, config.workflow_input_node).await;

    Ok(())
}

async fn wait_for_bus(socket_path: &str) {
    let path = std::path::Path::new(socket_path);
    let mut attempts = 0u32;
    while !path.exists() {
        if attempts == 0 {
            info!("cafe-comfy: waiting for bus at {}", socket_path);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
        if attempts > 60 {
            tracing::warn!("cafe-comfy: bus not ready after 30s, continuing anyway");
            break;
        }
    }
}
