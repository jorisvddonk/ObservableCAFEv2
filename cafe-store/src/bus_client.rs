use crate::db::Db;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{ServerMessage, SessionConfig};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

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

    // Check if bus is fresh (no sessions) while DB has data — restore if so.
    tokio::time::sleep(Duration::from_millis(500)).await;
    restore_from_db(&client, db).await;

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

/// On reconnect, attempt to restore any sessions from the local DB that
/// the bus doesn't already have. This rehydrates user sessions lost when
/// cafe-bus restarts.
async fn restore_from_db(client: &BusClient, db: &Arc<Db>) {
    let db_sessions = match db.list_sessions().await {
        Ok(s) => s,
        Err(e) => {
            warn!("cafe-store: failed to list DB sessions for restore: {}", e);
            return;
        }
    };
    if db_sessions.is_empty() {
        return;
    }

    let bus_sessions = match client.list_sessions().await {
        Ok(s) => s.iter().map(|s| s.session_id.clone()).collect::<Vec<_>>(),
        Err(e) => {
            warn!("cafe-store: failed to list bus sessions for restore: {}", e);
            return;
        }
    };

    for s in &db_sessions {
        if bus_sessions.contains(&s.session_id) {
            continue;
        }

        if let Err(e) = client
            .create_session(&s.session_id, &s.agent_id, SessionConfig::default())
            .await
        {
            // Race: another service may have created it between our list and create.
            warn!("cafe-store: create_session for {}: {}", s.session_id, e);
            continue;
        }
        info!("cafe-store: restored session {}", s.session_id);

        // Replay non-transient chunks into the bus.
        if let Ok(chunks) = db.load_history(&s.session_id).await {
            for chunk in &chunks {
                if let Err(e) = client.publish(&s.session_id, chunk.clone()).await {
                    warn!(
                        "cafe-store: failed to replay chunk {} for session {}: {}",
                        chunk.id, s.session_id, e
                    );
                    break;
                }
            }
            info!(
                "cafe-store: replayed {} chunks for session {}",
                chunks.len(),
                s.session_id
            );
        }
    }
}
