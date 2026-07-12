use anyhow::Result;
use cafe_bus::client;
use cafe_bus::config::Config;
use cafe_bus::registry::SessionRegistry;
use std::marker::Unpin;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::{Notify, RwLock};
use tracing::{info, warn};

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
    let conn_meta = client::ConnectionMetaRegistry::default();

    info!("cafe-bus listening on {}", config.socket_path);

    // Touch a ready file so process-compose readiness probe can detect us
    let ready_path = format!("{}.ready", config.socket_path);
    let _ = std::fs::write(&ready_path, b"");

    let shutdown = setup_shutdown_signal();

    // Optionally start iroh listener if configured
    let iroh_secret_key = std::env::var("CAFE_BUS_IROH_SECRET_KEY").ok();

    // Optional peer-ID allowlist for iroh connections.
    #[cfg(feature = "iroh-listener")]
    let allowlist: Option<Arc<cafe_bus::allowlist::Allowlist>> = {
        let db_path = std::env::var("CAFE_BUS_IROH_ALLOWLIST_DB").ok();
        let disabled = std::env::var("CAFE_BUS_IROH_ALLOWLIST_DISABLED")
            .ok()
            .map_or(false, |v| v == "1" || v == "true");
        match (db_path, disabled) {
            (Some(path), false) => match cafe_bus::allowlist::Allowlist::connect(&path).await {
                Ok(a) => {
                    let a = Arc::new(a);
                    a.spawn_refresh_task();
                    info!("cafe-bus iroh allowlist enabled ({})", path);
                    Some(a)
                }
                Err(e) => {
                    tracing::error!("failed to open iroh allowlist DB {}: {}", path, e);
                    None
                }
            },
            (Some(_), true) => {
                warn!("CAFE_BUS_IROH_ALLOWLIST_DISABLED set — iroh allowlist bypassed");
                None
            }
            (None, _) => None,
        }
    };

    #[cfg(feature = "iroh-listener")]
    let _iroh_handle: Option<tokio::task::JoinHandle<()>> = iroh_secret_key.as_ref().map(|key| {
        info!("cafe-bus starting iroh listener");
        let reg = registry.clone();
        let conns = connections.clone();
        let metas = conn_meta.clone();
        let key = key.clone();
        let allowlist = allowlist.clone();
        tokio::spawn(async move {
            if let Err(e) = run_iroh_listener(key, reg, conns, metas, allowlist).await {
                tracing::error!("iroh listener failed: {}", e);
            }
        })
    });

    #[cfg(not(feature = "iroh-listener"))]
    if iroh_secret_key.is_some() {
        tracing::warn!("CAFE_BUS_IROH_SECRET_KEY set but iroh-listener feature is not enabled");
    }

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let reg = registry.clone();
                        let conns = connections.clone();
                        let metas = conn_meta.clone();
                        let (reader, writer) = stream.into_split();
                        let writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send + 'static> = Box::new(writer);
                        tokio::spawn(client::handle_connection(reader, writer, reg, conns, metas));
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

#[cfg(feature = "iroh-listener")]
async fn run_iroh_listener(
    secret_key_hex: String,
    registry: Arc<RwLock<SessionRegistry>>,
    connections: client::ConnectionRegistry,
    conn_meta: client::ConnectionMetaRegistry,
    allowlist: Option<Arc<cafe_bus::allowlist::Allowlist>>,
) -> Result<()> {
    use std::str::FromStr;

    let secret_key = iroh::SecretKey::from_str(&secret_key_hex)?;

    let disable_relay = std::env::var("CAFE_BUS_IROH_DIRECT").ok().map_or(false, |v| v == "1");

    let relay_mode = if disable_relay {
        iroh::RelayMode::Disabled
    } else {
        iroh::RelayMode::Default
    };

    let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(secret_key)
        .alpns(vec![b"cafe-bus/0".to_vec()])
        .relay_mode(relay_mode);

    // Gate incoming connections against the peer-ID allowlist.
    if let Some(ref allowlist) = allowlist {
        builder = builder.hooks(cafe_bus::allowlist::AllowlistHook::new(allowlist.clone()));
    }

    let ep = builder.bind().await?;

    ep.online().await;

    let bus_addr = ep.addr();
    let bus_addr_json = serde_json::to_string(&bus_addr)?;
    info!("iroh listener ready, addr: {:?}", bus_addr);

    // Write the bus address to a file so clients can discover it
    let addr_file = std::env::var("CAFE_BUS_IROH_ADDR_FILE")
        .unwrap_or_else(|_| format!("{}.iroh-addr", std::env::var("CAFE_BUS_SOCKET").unwrap_or_default()));
    std::fs::write(&addr_file, &bus_addr_json)?;

    while let Some(incoming) = ep.accept().await {
        info!("iroh: incoming connection");
        match incoming.await {
            Ok(conn) => {
                info!("iroh: connection established");
                let reg = registry.clone();
                let conns = connections.clone();
                let metas = conn_meta.clone();
                tokio::spawn(async move {
                    info!("iroh: waiting for bidirectional stream");
                    match conn.accept_bi().await {
                        Ok((send, recv)) => {
                            info!("iroh: bidirectional stream accepted");
                            let writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send + 'static> =
                                Box::new(send);
                            client::handle_connection(
                                recv, writer, reg, conns, metas,
                            )
                            .await;
                        }
                        Err(e) => {
                            tracing::debug!("iroh: accept_bi error: {}", e);
                        }
                    }
                });
            }
            Err(e) => {
                tracing::debug!("iroh: incoming connection error: {}", e);
            }
        }
    }

    Ok(())
}
