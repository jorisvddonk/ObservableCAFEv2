use crate::db::Db;
use crate::storage::Storage;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

pub async fn run_gc_loop(
    storage: Arc<Storage>,
    db: Arc<Db>,
    gc_interval_secs: u64,
    gc_ttl_secs: u64,
) {
    // Startup: clean stale .writing files
    match storage.cleanup_stale_writes().await {
        Ok(ids) => {
            if !ids.is_empty() {
                info!("cafe-binary-store: cleaned {} stale .writing files", ids.len());
            }
        }
        Err(e) => warn!("cafe-binary-store: stale write cleanup error: {e}"),
    }

    let mut interval = tokio::time::interval(Duration::from_secs(gc_interval_secs));
    // Skip first tick (immediate), let startup settle
    interval.tick().await;

    loop {
        interval.tick().await;
        info!("cafe-binary-store: running GC");

        match db.gc_transient(gc_ttl_secs).await {
            Ok(ids) => {
                if ids.is_empty() {
                    continue;
                }
                for id in &ids {
                    if let Err(e) = storage.delete(id).await {
                        warn!("cafe-binary-store: GC failed to delete {id}: {e}");
                    }
                }
                info!(
                    "cafe-binary-store: GC deleted {} expired transient assets",
                    ids.len()
                );
            }
            Err(e) => warn!("cafe-binary-store: GC error: {e}"),
        }
    }
}
