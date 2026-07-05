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

/// Raise the per-process file descriptor limit so we don't hit "Too many open files".
fn raise_fd_limit() {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    let getret = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) };

    // Log current limits for debugging
    info!("FD limit: soft={} hard={} (getret={})", lim.rlim_cur, lim.rlim_max, getret);

    // On macOS from launchd: soft=256, hard=unlimited (18446744073709551615)
    // We try to set soft to the system max (92160) — this works if hard >= 92160.
    let target = 65536u64;
    if getret == 0 && lim.rlim_cur < target && lim.rlim_max >= target {
        lim.rlim_cur = target;
        let setret = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
        info!("FD limit set attempt: target={} setret={}", target, setret);
    } else if getret == 0 && lim.rlim_cur < target {
        // Hard limit is lower than target — try setting to hard limit
        lim.rlim_cur = lim.rlim_max;
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim); }
        info!("FD limit set to hard limit: {}", lim.rlim_max);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    raise_fd_limit();

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
    let connections = client::ConnectionRegistry::default();

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
                        let conns = connections.clone();
                        tokio::spawn(client::handle_client(stream, reg, conns));
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
