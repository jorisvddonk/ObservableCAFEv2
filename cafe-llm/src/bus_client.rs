use crate::backends::LlmBackend;
use crate::evaluator::run_session;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{Chunk, ServerMessage, SessionConfig};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

const REGISTRY_SESSION_ID: &str = "_cafe_llm_registry";

pub async fn run_with_reconnect(
    socket_path: String,
    backend: Arc<dyn LlmBackend>,
    default_model: String,
) {
    cafe_sdk::bus::run_with_reconnect("cafe-llm", move || {
        let socket = socket_path.clone();
        let backend = backend.clone();
        let model = default_model.clone();
        async move { connect_and_run(&socket, backend, &model).await }
    })
    .await;
}

async fn connect_and_run(
    socket_path: &str,
    backend: Arc<dyn LlmBackend>,
    default_model: &str,
) -> anyhow::Result<()> {
    info!("cafe-llm: starting (subscribe-all mode) on {}", socket_path);

    let client = BusClient::new(socket_path);

    if let Ok(models) = backend.list_models().await {
        if !models.is_empty() {
            info!("cafe-llm: discovered {} models", models.len());
        }
        publish_model_registry(&client, &models).await?;
    }

    // Subscribe to all sessions — snapshot replays history + sends SessionCreated for existing sessions,
    // and the event listener forwards SessionCreated for new sessions created later.
    let mut rx = client.subscribe_all().await?;

    let mut model_tick: u64 = 0;

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(ServerMessage::SessionCreated { session_id, .. }) => {
                        info!("cafe-llm: new session via SubscribeAll: {}", session_id);
                        spawn_session(session_id, socket_path, &backend, default_model);
                    }
                    Some(_) => {}
                    None => {
                        info!("cafe-llm: SubscribeAll stream ended, will reconnect");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                model_tick += 1;
                if model_tick >= 30 {
                    model_tick = 0;
                    if let Ok(models) = backend.list_models().await {
                        publish_model_registry(&client, &models).await?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn spawn_session(
    session_id: String,
    socket_path: &str,
    backend: &Arc<dyn LlmBackend>,
    default_model: &str,
) {
    let sid = session_id;
    let sp = socket_path.to_string();
    let b = backend.clone();
    let m = default_model.to_string();
    tokio::spawn(async move {
        if let Err(e) = run_session(sid.clone(), sp, b, m).await {
            warn!("cafe-llm: session {} evaluator error: {}", sid, e);
        }
    });
}

async fn publish_model_registry(client: &BusClient, models: &[String]) -> anyhow::Result<()> {
    client
        .create_session(REGISTRY_SESSION_ID, "_llm_registry", SessionConfig::default())
        .await?;

    let models_json = serde_json::to_string(models)?;
    let chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation("config.type", "runtime")
        .with_annotation("config.available_models", models_json);

    client.publish(REGISTRY_SESSION_ID, chunk).await?;

    Ok(())
}
