mod auth;
mod binary_ref;
mod config;
mod db;
mod handlers;
mod route_registry;
mod router;
mod sse;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use cafe_http_proxy_sdk::{parse_registration, parse_response, PROXY_SESSION};
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, ServerMessage};
use config::Config;
use db::Db;
use handlers::proxy::{ProxiedResponse, ProxyState};
use route_registry::{spawn_gc, RouteRegistryInner};
use tokio::sync::{oneshot, RwLock};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct AppState {
    pub bus: BusClient,
    pub db: Arc<Db>,
    pub proxy_state: Arc<ProxyState>,
}

impl axum::extract::FromRef<AppState> for ProxyState {
    fn from_ref(state: &AppState) -> Self {
        state.proxy_state.as_ref().clone()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    let db = Arc::new(Db::connect(&config.db_path).await?);
    db.migrate().await?;

    let admin_token = db.ensure_admin_token(config.admin_token.as_deref()).await?;
    println!("==============================================");
    println!("ADMIN TOKEN (save this): {}", admin_token);
    println!("==============================================");

    let bus = BusClient::new(config.socket_path.clone());

    // Set up the HTTP proxy route registry
    let registry = Arc::new(RouteRegistryInner::new(
        config.proxy_max_body_size,
        config.proxy_gc_interval_secs,
        config.proxy_stale_purge_secs,
    ));
    spawn_gc(registry.clone());

    let pending: Arc<RwLock<HashMap<String, oneshot::Sender<Result<ProxiedResponse, String>>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let proxy_state = Arc::new(ProxyState {
        registry: registry.clone(),
        bus: bus.clone(),
        pending: pending.clone(),
    });

    // Subscribe to the proxy session
    let sub_bus = bus.clone();
    let sub_registry = registry.clone();
    let sub_pending = pending.clone();
    tokio::spawn(async move {
        if let Err(e) = run_proxy_subscriber(sub_bus, sub_registry, sub_pending).await {
            error!("proxy subscriber exited: {}", e);
        }
    });

    let state = AppState {
        bus,
        db,
        proxy_state: proxy_state.clone(),
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

/// Subscribe to `cafe-server.http-proxy`, handle registrations and RPC responses.
async fn run_proxy_subscriber(
    bus: BusClient,
    registry: Arc<RouteRegistryInner>,
    pending: Arc<RwLock<HashMap<String, oneshot::Sender<Result<ProxiedResponse, String>>>>>,
) -> Result<()> {
    // We use subscribe_all and filter by session — avoids history replay issues
    // since the proxy session has only transient chunks.
    let mut rx = bus.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        let (sid, chunk) = match &msg {
            ServerMessage::Chunk { session_id, chunk } => (session_id.as_str(), chunk),
            _ => continue,
        };
        if sid != PROXY_SESSION {
            continue;
        }

        // Route registration (no direct_to → broadcast)
        if let Some(reg) = parse_registration(chunk) {
            let conn_id = chunk
                .annotations
                .get(keys::CAFE_SOURCE_CONNECTION)
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            info!(
                "registered route {} {} from connection {}",
                reg.methods.join(","),
                reg.pattern,
                conn_id
            );
            registry
                .upsert(&reg.pattern, reg.methods, conn_id)
                .await;
            continue;
        }

        // RPC response with direct_to = no direct to check, but it's directed to us
        // We check if the chunk has a direct_to — if it doesn't, it might be a registration.
        // If it does and it's NOT targeting us, skip.
        // Actually, subscribe_all gives us all chunks. We need to check direct_to.
        if chunk
            .annotations
            .get(keys::CAFE_DIRECT_TO)
            .and_then(|v| v.as_str())
            .is_some()
        {
            // Only process if targeted at us — but we don't know our own connection ID.
            // Instead, check if there's a pending oneshot for this call_id.
            if let Some(resp) = parse_response(chunk) {
                // Find the call_id from the JSON-RPC response
                if let Some(rpc_resp) = chunk.as_rpc_response() {
                    let call_id = &rpc_resp.id;
                    let mut p = pending.write().await;
                    if let Some(sender) = p.remove(call_id) {
                        drop(p);
                        let proxied = ProxiedResponse {
                            status: resp.status,
                            headers: resp.headers,
                            body: cafe_http_proxy_sdk::decode_body(&resp.body).unwrap_or_default(),
                        };
                        let _ = sender.send(Ok(proxied));
                    }
                }
            }
        }
    }

    Ok(())
}
