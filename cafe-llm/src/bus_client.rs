use crate::backends::LlmBackend;
use crate::evaluator::run_session;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{Chunk, SessionConfig};
use std::collections::HashSet;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionKey {
    session_id: String,
    model: Option<String>,
    system_prompt: Option<String>,
}

async fn connect_and_run(
    socket_path: &str,
    backend: Arc<dyn LlmBackend>,
    default_model: &str,
) -> anyhow::Result<()> {
    info!("cafe-llm: starting session poller on {}", socket_path);

    let client = BusClient::new(socket_path);
    let mut known_sessions: HashSet<SessionKey> = HashSet::new();

    if let Ok(models) = backend.list_models().await {
        if !models.is_empty() {
            info!("cafe-llm: discovered {} models", models.len());
        }
        publish_model_registry(&client, &models).await?;
    }

    let mut model_tick: u64 = 0;

    loop {
        match client.list_sessions().await {
            Ok(sessions) => {
                let mut current_keys: HashSet<SessionKey> = HashSet::new();

                for info in &sessions {
                    let key = SessionKey {
                        session_id: info.session_id.clone(),
                        model: None,
                        system_prompt: None,
                    };

                    current_keys.insert(key.clone());

                    if !known_sessions.contains(&key) {
                        info!("cafe-llm: discovered session {}", info.session_id);
                        known_sessions.insert(key);

                        let sid = info.session_id.clone();
                        let sp = socket_path.to_string();
                        let b = backend.clone();
                        let m = default_model.to_string();
                        tokio::spawn(async move {
                            if let Err(e) = run_session(sid.clone(), sp, b, m).await {
                                warn!("cafe-llm: session {} evaluator error: {}", sid, e);
                            }
                        });
                    }
                }

                for key in known_sessions.clone() {
                    if !current_keys.contains(&key) {
                        info!("cafe-llm: session removed {}", key.session_id);
                        known_sessions.remove(&key);
                    }
                }
            }
            Err(e) => {
                warn!("cafe-llm: list_sessions error: {}", e);
            }
        }

        model_tick += 1;
        if model_tick >= 30 {
            model_tick = 0;
            if let Ok(models) = backend.list_models().await {
                publish_model_registry(&client, &models).await?;
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
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
