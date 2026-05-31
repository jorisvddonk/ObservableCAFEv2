use crate::db::Db;
use anyhow::Result;
use cafe_types::{ClientMessage, ServerMessage};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(socket_path: String, db: Arc<Db>) {
    loop {
        match connect_and_run(&socket_path, &db).await {
            Ok(()) => {
                info!("cafe-store: clean shutdown");
                break;
            }
            Err(e) => {
                warn!("bus connection lost: {}. Reconnecting in 2s", e);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn connect_and_run(socket_path: &str, db: &Arc<Db>) -> Result<()> {
    let stream = UnixStream::connect(socket_path).await?;
    info!("cafe-store: connected to bus at {}", socket_path);

    let (reader, mut writer) = stream.into_split();

    // Subscribe to all sessions
    let sub_msg = serde_json::to_string(&ClientMessage::SubscribeAll)? + "\n";
    writer.write_all(sub_msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("cafe-store: invalid message from bus: {}", e);
                continue;
            }
        };

        match msg {
            ServerMessage::SessionCreated {
                session_id,
                agent_id,
            } => {
                if let Err(e) = db.upsert_session(&session_id, &agent_id, false).await {
                    error!("cafe-store: failed to upsert session {}: {}", session_id, e);
                }
            }
            ServerMessage::SessionDeleted { session_id } => {
                if let Err(e) = db.delete_session(&session_id).await {
                    error!("cafe-store: failed to delete session {}: {}", session_id, e);
                }
            }
            ServerMessage::Chunk { session_id, chunk } => {
                // Ensure session exists (bus may not have sent SessionCreated for pre-existing sessions)
                let _ = db.upsert_session(&session_id, "unknown", false).await;
                if let Err(e) = db.insert_chunk(&session_id, &chunk).await {
                    error!(
                        "cafe-store: failed to insert chunk {} for session {}: {}",
                        chunk.id, session_id, e
                    );
                }
            }
            ServerMessage::HistoryComplete { session_id, count } => {
                info!(
                    "cafe-store: history replay complete for session {} ({} chunks)",
                    session_id, count
                );
            }
            _ => {}
        }
    }

    Ok(())
}
