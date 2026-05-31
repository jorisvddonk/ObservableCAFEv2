mod backends;
mod bus_client;
mod config;
mod context;
mod evaluator;

use anyhow::Result;
use backends::{ollama::OllamaBackend, openai::OpenAiBackend, LlmBackend};
use config::Config;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    let backend: Arc<dyn LlmBackend> = match config.backend.as_str() {
        "openai" => {
            info!("cafe-llm: using OpenAI-compatible backend at {}", config.openai_url);
            Arc::new(OpenAiBackend::new(
                config.openai_url.clone(),
                config.openai_api_key.clone(),
            ))
        }
        _ => {
            info!("cafe-llm: using Ollama backend at {}", config.ollama_url);
            Arc::new(OllamaBackend::new(config.ollama_url.clone()))
        }
    };

    let default_model = match config.backend.as_str() {
        "openai" => config.openai_model.clone(),
        _ => config.ollama_model.clone(),
    };

    info!("cafe-llm: default model: {}", default_model);

    let socket = config.socket_path.clone();
    let b = backend.clone();
    let m = default_model.clone();

    tokio::spawn(async move {
        bus_client::run_with_reconnect(socket, b, m).await;
    });

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = async {
            let mut sigterm = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate()
            ).expect("failed to register SIGTERM");
            sigterm.recv().await;
        } => {}
    }

    info!("cafe-llm: shutting down");
    Ok(())
}
