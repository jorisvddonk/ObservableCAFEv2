mod bus_client;
mod config;
mod db;

use anyhow::Result;
use config::Config;
use db::Db;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let db = Arc::new(Db::connect(&config.db_path).await?);
    db.migrate().await?;

    let sessions = db.session_count().await?;
    let chunks = db.chunk_count().await?;
    info!(
        "cafe-store: database ready — {} sessions, {} chunks",
        sessions, chunks
    );

    // Handle SIGTERM gracefully
    let db2 = db.clone();
    let socket = config.socket_path.clone();
    tokio::spawn(async move {
        bus_client::run_with_reconnect(socket, db2).await;
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

    info!("cafe-store: shutting down");
    Ok(())
}
