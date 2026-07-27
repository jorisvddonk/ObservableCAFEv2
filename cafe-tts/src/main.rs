mod config;
mod speech_server;
mod voicebox;
mod worker;

use config::{Config, TtsBackend};
use speech_server::{SpeechServerClient, TtsClient};
use std::time::Duration;
use tracing::info;
use voicebox::VoiceboxClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    let client = match config.backend {
        TtsBackend::Voicebox => {
            info!(
                "cafe-tts: starting — bus={} backend=voicebox url={}",
                config.socket_path, config.voicebox_url
            );
            TtsClient::Voicebox(VoiceboxClient::new(&config.voicebox_url))
        }
        TtsBackend::SpeechServer => {
            info!(
                "cafe-tts: starting — bus={} backend=speech-server url={}",
                config.socket_path, config.speech_server_url
            );
            TtsClient::SpeechServer(SpeechServerClient::new(&config.speech_server_url))
        }
    };

    // Wait for the bus socket to appear before trying to connect
    wait_for_bus(&config.socket_path).await;

    worker::run_with_reconnect(config.socket_path, client).await;

    Ok(())
}

async fn wait_for_bus(socket_path: &str) {
    let path = std::path::Path::new(socket_path);
    let mut attempts = 0u32;
    while !path.exists() {
        if attempts == 0 {
            info!("cafe-tts: waiting for bus at {}", socket_path);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
        if attempts > 60 {
            tracing::warn!("cafe-tts: bus not ready after 30s, continuing anyway");
            break;
        }
    }
}
