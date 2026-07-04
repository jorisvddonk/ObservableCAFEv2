use crate::db::Db;
use crate::jwt;
use crate::storage::Storage;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, ContentType, ServerMessage, SubscribeFilter};
use std::sync::Arc;
use tracing::{error, info, warn};

pub async fn run(
    cfg: crate::config::Config,
    storage: Arc<Storage>,
    db: Arc<Db>,
    jwt_key: Arc<Vec<u8>>,
) {
    let bus = BusClient::new(&cfg.bus_socket);

    // Subscribe only for BinaryRef chunks
    let filter = SubscribeFilter {
        sessions: None,
        agents: None,
        content_types: Some(vec![ContentType::BinaryRef]),
        annotations: None,
    };

    let mut rx = match bus.subscribe_filtered(filter).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!("cafe-binary-store: subscribe_filtered failed: {e}, will retry on reconnect");
            return;
        }
    };

    info!("cafe-binary-store: connected to bus at {}", cfg.bus_socket);

    while let Some(msg) = rx.recv().await {
        match msg {
            ServerMessage::Chunk {
                session_id,
                chunk,
            } => {
                let chunk_id = chunk.id.clone();
                // Track asset in DB
                let is_transient = chunk.is_transient();
                if let Err(e) = db.insert_asset(&chunk_id, &session_id, is_transient).await {
                    error!("cafe-binary-store: failed to insert asset: {e}");
                    continue;
                }

                // Check if the chunk includes a byte_size — actual size comes from the POST
                // Generate write JWT and send via direct mutation
                let write_token = match jwt::sign_write(&chunk_id, cfg.write_ttl, &jwt_key) {
                    Ok(t) => t,
                    Err(e) => {
                        error!("cafe-binary-store: failed to sign write JWT: {e}");
                        continue;
                    }
                };

                // Find source_connection for direct reply
                let source_conn = chunk.get_annotation::<String>(keys::CAFE_SOURCE_CONNECTION);
                let reply_target = match source_conn {
                    Some(ref c) => c.clone(),
                    None => {
                        warn!("cafe-binary-store: BinaryRef chunk {} has no source.connection", chunk_id);
                        continue;
                    }
                };

                let port = cfg.port;
                let base_url = format!("http://0.0.0.0:{port}/api/binary/{chunk_id}");
                let write_url = format!("{base_url}");

                // Build mutation with write credentials
                let mut mutation = cafe_sdk::Chunk::mutation(&chunk_id, "com.nominal.cafe-binary-store");
                mutation = mutation
                    .with_annotation(keys::CAFE_BINARY_WRITE_URL, &write_url)
                    .with_annotation(keys::CAFE_BINARY_WRITE_TOKEN, &write_token)
                    .as_transient();

                // Also include the base read URL so the producer can construct it
                let read_url = format!("http://0.0.0.0:{port}/api/binary/{chunk_id}");
                mutation = mutation.with_annotation(keys::CAFE_BINARY_READ_URL, &read_url);

                // Send direct mutation to the producer only
                if let Err(e) = bus
                    .publish_direct(&reply_target, &session_id, mutation)
                    .await
                {
                    warn!("cafe-binary-store: failed to publish write credentials: {e}");
                }

                // The read JWT will be published when the POST starts (in main.rs via channel)
            }
            ServerMessage::SessionDeleted { session_id } => {
                // Cascade delete non-transient assets for this session
                match db.delete_session_non_transient(&session_id).await {
                    Ok(ids) => {
                        for id in ids {
                            storage.delete(&id).await.ok();
                            info!("cafe-binary-store: deleted file for session {session_id} chunk {id}");
                        }
                    }
                    Err(e) => {
                        error!("cafe-binary-store: failed to delete session assets: {e}");
                    }
                }
            }
            _ => {}
        }
    }
}
