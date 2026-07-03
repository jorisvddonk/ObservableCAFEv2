use crate::db::Db;
use cafe_sdk::bus::BusClient;
use cafe_sdk::ServerMessage;
use std::sync::Arc;
use tracing::{error, info};

pub async fn run(socket_path: String, db: Arc<Db>) {
    cafe_sdk::bus::run_with_reconnect("cafe-store", move || {
        let socket = socket_path.clone();
        let db = db.clone();
        async move { connect_and_run(&socket, &db).await }
    })
    .await;
}

async fn connect_and_run(socket_path: &str, db: &Arc<Db>) -> anyhow::Result<()> {
    let client = BusClient::new(socket_path);
    let mut rx = client.subscribe_all().await?;
    info!("cafe-store: connected to bus at {}", socket_path);

    while let Some(msg) = rx.recv().await {
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
                // Transient chunks are never persisted
                if chunk.is_transient() {
                    continue;
                }
                // RPC protocol chunks are internal — never persisted
                if chunk.as_rpc_request().is_some() {
                    continue;
                }
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
