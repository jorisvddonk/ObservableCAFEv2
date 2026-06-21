use crate::backends::LlmBackend;
use crate::evaluator::run_session;
use anyhow::Result;
use cafe_types::Chunk;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{info, warn};

const REGISTRY_SESSION_ID: &str = "_cafe_llm_registry";

pub async fn run_with_reconnect(
    socket_path: String,
    backend: Arc<dyn LlmBackend>,
    default_model: String,
) {
    loop {
        match connect_and_run(&socket_path, backend.clone(), default_model.clone()).await {
            Ok(()) => {
                info!("cafe-llm: clean shutdown");
                break;
            }
            Err(e) => {
                warn!("cafe-llm: bus connection lost: {}. Reconnecting in 2s", e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
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
    default_model: String,
) -> Result<()> {
    info!("cafe-llm: starting session poller on {}", socket_path);

    let mut known_sessions: HashSet<SessionKey> = HashSet::new();

    if let Ok(models) = backend.list_models().await {
        if !models.is_empty() {
            info!("cafe-llm: discovered {} models", models.len());
        }
        publish_model_registry(socket_path, &models).await?;
    }

    let mut model_tick: u64 = 0;

    loop {
        match list_session_ids(socket_path).await {
            Ok(sessions) => {
                let mut current_keys: HashSet<SessionKey> = HashSet::new();

                for info in sessions {
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
                        let m = default_model.clone();
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
                publish_model_registry(socket_path, &models).await?;
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn publish_model_registry(socket_path: &str, models: &[String]) -> Result<()> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    let create_msg = serde_json::to_string(&cafe_types::ClientMessage::CreateSession {
        session_id: REGISTRY_SESSION_ID.into(),
        agent_id: "_llm_registry".into(),
        config: cafe_types::SessionConfig::default(),
    })? + "\n";
    writer.write_all(create_msg.as_bytes()).await?;

    let _ = BufReader::new(reader).lines().next_line().await;

    let models_json = serde_json::to_string(models)?;
    let chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation("config.type", "runtime")
        .with_annotation("config.available_models", models_json);

    let pub_msg = serde_json::to_string(&cafe_types::ClientMessage::Publish {
        session_id: REGISTRY_SESSION_ID.into(),
        chunk,
    })? + "\n";
    writer.write_all(pub_msg.as_bytes()).await?;

    Ok(())
}

async fn list_session_ids(socket_path: &str) -> Result<Vec<cafe_types::SessionInfo>> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    let list_msg = serde_json::to_string(&cafe_types::ClientMessage::ListSessions)? + "\n";
    writer.write_all(list_msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if let Ok(cafe_types::ServerMessage::SessionsList { sessions }) =
            serde_json::from_str(&line)
        {
            return Ok(sessions);
        }
    }

    Ok(vec![])
}
