mod client;
mod config;
mod registry;
mod session;

use anyhow::Result;
use config::Config;
use registry::SessionRegistry;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::{Notify, RwLock};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    // Remove stale socket if it exists
    let _ = std::fs::remove_file(&config.socket_path);

    let listener = UnixListener::bind(&config.socket_path)?;

    // Restrict socket to owner only
    std::fs::set_permissions(
        &config.socket_path,
        std::fs::Permissions::from_mode(0o600),
    )?;

    let registry = Arc::new(RwLock::new(SessionRegistry::new()));

    info!("cafe-bus listening on {}", config.socket_path);

    // Touch a ready file so process-compose readiness probe can detect us
    let ready_path = format!("{}.ready", config.socket_path);
    let _ = std::fs::write(&ready_path, b"");

    let shutdown = setup_shutdown_signal();

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let reg = registry.clone();
                        tokio::spawn(client::handle_client(stream, reg));
                    }
                    Err(e) => {
                        tracing::error!("accept error: {}", e);
                    }
                }
            }
            _ = shutdown.notified() => {
                info!("cafe-bus shutting down");
                break;
            }
        }
    }

    // Clean up socket files
    let _ = std::fs::remove_file(&config.socket_path);
    let _ = std::fs::remove_file(&ready_path);

    Ok(())
}

fn setup_shutdown_signal() -> Arc<Notify> {
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();
    tokio::spawn(async move {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        notify2.notify_one();
    });
    notify
}
