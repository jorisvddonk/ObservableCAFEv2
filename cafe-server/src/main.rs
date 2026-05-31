mod auth;
mod bus_client;
mod config;
mod db;
mod handlers;
mod router;
mod sse;

use anyhow::Result;
use bus_client::BusClient;
use config::Config;
use db::Db;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub bus: BusClient,
    pub db: Arc<Db>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    let db = Arc::new(Db::connect(&config.db_path).await?);
    db.migrate().await?;

    // Ensure admin token exists
    let admin_token = db.ensure_admin_token(config.admin_token.as_deref()).await?;
    println!("==============================================");
    println!("ADMIN TOKEN (save this): {}", admin_token);
    println!("==============================================");

    let bus = BusClient::new(config.socket_path.clone());

    let state = AppState {
        bus,
        db,
    };
    let app = router::build_router(state);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("cafe-server listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("cafe-server: shutting down");
    Ok(())
}

async fn shutdown_signal() {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = async {
            let mut sigterm = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate()
            ).expect("failed to register SIGTERM");
            sigterm.recv().await;
        } => {}
    }
}
