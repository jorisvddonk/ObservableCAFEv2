use crate::backends::LlmBackend;
use crate::evaluator::run_session;
use anyhow::Result;
use cafe_types::{ClientMessage, ServerMessage};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{info, warn};

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

async fn connect_and_run(
    socket_path: &str,
    backend: Arc<dyn LlmBackend>,
    default_model: String,
) -> Result<()> {
    let stream = UnixStream::connect(socket_path).await?;
    info!("cafe-llm: connected to bus at {}", socket_path);

    let (reader, mut writer) = stream.into_split();

    // Subscribe to session list to discover sessions
    let list_msg = serde_json::to_string(&ClientMessage::ListSessions)? + "\n";
    writer.write_all(list_msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    let mut known_sessions: HashSet<String> = HashSet::new();

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("cafe-llm: invalid message: {}", e);
                continue;
            }
        };

        match msg {
            ServerMessage::SessionsList { sessions } => {
                for session in sessions {
                    if !known_sessions.contains(&session.session_id) {
                        known_sessions.insert(session.session_id.clone());
                        let sid = session.session_id.clone();
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
            }
            ServerMessage::SessionCreated {
                session_id,
                agent_id: _,
            } => {
                if !known_sessions.contains(&session_id) {
                    known_sessions.insert(session_id.clone());
                    let sid = session_id.clone();
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
            ServerMessage::SessionDeleted { session_id } => {
                known_sessions.remove(&session_id);
            }
            _ => {}
        }
    }

    Ok(())
}
