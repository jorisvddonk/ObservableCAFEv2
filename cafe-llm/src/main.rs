mod backends;
mod bus_client;
mod config;
mod context;
mod evaluator;

use anyhow::Result;
use backends::{
    ollama::OllamaBackend, openai::OpenAiBackend, opencode_go::OpenCodeGoBackend, router::BackendRouter,
    LlmBackend,
};
use config::Config;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    // Construct every backend regardless of the process default so a session
    // can select one at runtime via `config.llm.backend` (ADR-134). Each client
    // is cheap: just a `reqwest::Client` plus base URL.
    let ollama: Arc<dyn LlmBackend> =
        Arc::new(OllamaBackend::new(config.ollama_url.clone()));
    let openai: Arc<dyn LlmBackend> = Arc::new(OpenAiBackend::new(
        config.openai_url.clone(),
        config.openai_api_key.clone(),
        config.model_list_urls.clone(),
    ));
    let opencode_go: Arc<dyn LlmBackend> = Arc::new(OpenCodeGoBackend::new(
        config.opencode_go_url.clone(),
        config.opencode_api_key.clone(),
    ));

    let backend: Arc<dyn LlmBackend> = Arc::new(BackendRouter::new(
        config.backend.clone(),
        vec![
            ("ollama".into(), ollama, Some(config.ollama_model.clone())),
            ("openai".into(), openai, Some(config.openai_model.clone())),
            (
                "opencode-go".into(),
                opencode_go,
                Some(config.opencode_go_model.clone()),
            ),
        ],
    ));

    info!(
        "cafe-llm: backend router ready (default: {}, ollama: {}, openai: {}, opencode-go: {})",
        config.backend, config.ollama_url, config.openai_url, config.opencode_go_url
    );

    let default_model = match config.backend.as_str() {
        "openai" => config.openai_model.clone(),
        "opencode-go" | "opencode_go" => config.opencode_go_model.clone(),
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
