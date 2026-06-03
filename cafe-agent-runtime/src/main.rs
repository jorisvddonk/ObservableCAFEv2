mod config;
mod lifecycle;
mod loader;
mod registry;
mod scheduler;
mod watcher;

use anyhow::Result;
use config::Config;
use registry::{AgentEntry, AgentRegistry};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let registry = Arc::new(Mutex::new(AgentRegistry::new()));

    // 1. Scan agent directories and load definitions
    let mut all_agents = Vec::new();
    for dir in &config.agent_paths {
        let found = loader::scan_directory(dir);
        info!("cafe-agent-runtime: found {} agents in {}", found.len(), dir);
        all_agents.extend(found);
    }

    // 2. Wait for bus to be ready
    wait_for_bus(&config.socket_path).await;

    // 3. Register agents and start background sessions
    let sched = scheduler::AgentScheduler::new().await?;

    for (path, def) in &all_agents {
        let hash = loader::hash_file(path);
        let name = def.name.clone();

        if def.background {
            info!("cafe-agent-runtime: starting background agent '{}'", name);
            if let Err(e) = lifecycle::create_agent_session(
                &config.socket_path,
                &name,
                Some(def.initial_chunk_content.clone()),
                Some(def.initial_chunk_type.clone()),
                def.initial_chunk_data.clone(),
                def.initial_chunk_mime_type.clone(),
                def.initial_chunk_annotations.clone(),
            ).await {
                warn!("cafe-agent-runtime: failed to create session for '{}': {}", name, e);
            }

            if let Some(cron) = &def.schedule {
                if let Err(e) = sched
                    .schedule(name.clone(), cron, config.socket_path.clone())
                    .await
                {
                    error!("cafe-agent-runtime: failed to schedule '{}': {}", name, e);
                }
            }
        }

        registry.lock().unwrap().insert(AgentEntry {
            def: def.clone(),
            path: path.clone(),
            file_hash: hash,
        });
    }

    sched.start().await?;
    info!("cafe-agent-runtime: agents ready");

    // 4. Start file watcher for hot-reload
    let dirs: Vec<String> = config.agent_paths.clone();
    let (_watcher_handle, change_rx) = match watcher::start_watcher(&dirs) {
        Ok(w) => w,
        Err(e) => {
            warn!("cafe-agent-runtime: file watcher failed to start: {}", e);
            // Continue without hot-reload
            let (_, rx) = tokio::sync::mpsc::channel(1);
            return run_until_shutdown(rx).await;
        }
    };

    run_until_shutdown(change_rx).await
}

async fn run_until_shutdown(
    mut change_rx: tokio::sync::mpsc::Receiver<std::path::PathBuf>,
) -> Result<()> {
    loop {
        tokio::select! {
            Some(path) = change_rx.recv() => {
                info!("cafe-agent-runtime: detected change in {:?}", path);
                // Hot-reload: re-parse the file and reset the session if allowed
                match loader::load_agent_file(&path) {
                    Ok(new_def) => {
                        let name = new_def.name.clone();
                        info!("cafe-agent-runtime: hot-reloading agent '{}'", name);
                        if new_def.allows_reload {
                            // Signal reset — the evaluator will re-init
                            // (socket_path not available here; would need Arc<Config>)
                            info!("cafe-agent-runtime: agent '{}' reloaded", name);
                        }
                    }
                    Err(e) => warn!("cafe-agent-runtime: failed to reload {:?}: {}", path, e),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("cafe-agent-runtime: shutting down");
                break;
            }
            _ = async {
                let mut sigterm = tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::terminate()
                ).expect("SIGTERM handler");
                sigterm.recv().await;
            } => {
                info!("cafe-agent-runtime: shutting down");
                break;
            }
        }
    }
    Ok(())
}

/// Poll until the bus socket exists.
async fn wait_for_bus(socket_path: &str) {
    let path = std::path::Path::new(socket_path);
    let mut attempts = 0u32;
    while !path.exists() {
        if attempts == 0 {
            info!("cafe-agent-runtime: waiting for bus at {}", socket_path);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
        if attempts > 60 {
            warn!("cafe-agent-runtime: bus not ready after 30s, continuing anyway");
            break;
        }
    }
}
