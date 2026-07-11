mod config;
mod executor;
mod lifecycle;
mod loader;
mod registry;
mod scheduler;
mod session_loop;
mod tool_detector;
mod tool_executor;
mod watcher;

use anyhow::Result;
use cafe_sdk::{ServerMessage, StepDef};
use config::Config;
use executor::PipelineExecutor;
use registry::{AgentEntry, AgentRegistry};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tracing::{error, info, warn};

/// Everything the poller needs to know about an agent with RPC steps.
#[derive(Clone)]
struct AgentPipelineInfo {
    steps: Vec<StepDef>,
    rpc_timeout_secs: u64,
    max_pipeline_depth: u32,
    /// initial_chunk type ("null", "text", …) — used to seed config into new sessions
    initial_chunk_type: String,
    initial_chunk_annotations: std::collections::HashMap<String, serde_json::Value>,
}

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
    if let Err(e) = cafe_sdk::bus::wait_for_bus(&config.socket_path, Duration::from_millis(500), 60).await {
        warn!("cafe-agent-runtime: bus not ready after 30s, continuing anyway: {e}");
    }

    // 3. Register agents and start background sessions
    let sched = scheduler::AgentScheduler::new().await?;

    // Build a map of agent_id → pipeline info for quick lookup in the poller
    let mut agent_pipelines: HashMap<String, AgentPipelineInfo> = HashMap::new();

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
                def.ephemeral_keepalive_secs,
                def.ephemeral_count_role.clone(),
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

        // Record pipeline for any agent that has RPC steps
        let has_rpc_steps = def.steps.iter().any(|s| {
            !matches!(s.step_type.as_str(), "role-annotator" | "trust-filter" | "tool-detector" | "tool-executor")
        });
        if has_rpc_steps {
            agent_pipelines.insert(name.clone(), AgentPipelineInfo {
                steps: def.steps.clone(),
                rpc_timeout_secs: def.rpc_timeout_secs,
                max_pipeline_depth: def.max_pipeline_depth,
                initial_chunk_type: def.initial_chunk_type.clone(),
                initial_chunk_annotations: def.initial_chunk_annotations.clone(),
            });
        }

        registry.lock().unwrap_or_else(PoisonError::into_inner).insert(AgentEntry {
            def: def.clone(),
            path: path.clone(),
            file_hash: hash,
        });
    }

    sched.start().await?;
    info!("cafe-agent-runtime: agents ready");

    // 4. Start pipeline session poller — discovers sessions whose agent has RPC
    //    steps and spawns a pipeline watcher per session (same pattern as cafe-llm).
    if !agent_pipelines.is_empty() {
        let sp = config.socket_path.clone();
        let pipelines = Arc::new(agent_pipelines);
        tokio::spawn(async move {
            run_pipeline_subscriber(sp, pipelines).await;
        });
    }
    // 5. Start file watcher for hot-reload
    let dirs: Vec<String> = config.agent_paths.clone();
    let (_watcher_handle, change_rx) = match watcher::start_watcher(&dirs) {
        Ok(w) => w,
        Err(e) => {
            warn!("cafe-agent-runtime: file watcher failed to start: {}", e);
            let (_, rx) = tokio::sync::mpsc::channel(1);
            return run_until_shutdown(rx).await;
        }
    };

    run_until_shutdown(change_rx).await
}

/// Subscribe to all sessions via SubscribeAll. For each session whose agent_id
/// has RPC steps:
///  1. Publish the agent's initial config chunk (if it's a null config chunk) so
///     resolve_session_config can find TTS/LLM settings in the session history.
///  2. Spawn a run_session_pipeline task to watch for LLM completions and fire RPC steps.
async fn run_pipeline_subscriber(
    socket_path: String,
    agent_pipelines: Arc<HashMap<String, AgentPipelineInfo>>,
) {
    let client = cafe_sdk::bus::BusClient::unix(&socket_path);
    let mut rx = match client.subscribe_all().await {
        Ok(rx) => rx,
        Err(e) => {
            warn!("cafe-agent-runtime: subscribe_all failed: {}", e);
            return;
        }
    };

    while let Some(msg) = rx.recv().await {
        let (session_id, agent_id) = match msg {
            ServerMessage::SessionCreated { session_id, agent_id } => (session_id, agent_id),
            _ => continue,
        };

        if !agent_pipelines.contains_key(&agent_id) {
            info!(
                "cafe-agent-runtime: ignoring session {} (agent {} not in pipeline map)",
                session_id, agent_id
            );
            continue;
        }

        let pipeline_info = agent_pipelines.get(&agent_id).unwrap();
        info!(
            "cafe-agent-runtime: attaching pipeline to session {} (agent {})",
            session_id, agent_id
        );

        let sid = session_id.clone();
        let sp = socket_path.clone();
        let agent_id2 = agent_id.clone();

        // Publish the initial config chunk so resolve_session_config
        // picks up TTS/LLM settings for user-created sessions.
        if pipeline_info.initial_chunk_type == "null" && !pipeline_info.initial_chunk_annotations.is_empty() {
            let annotations = pipeline_info.initial_chunk_annotations.clone();
            let client = client.clone();
            let sid2 = sid.clone();
            tokio::spawn(async move {
                let mut chunk = cafe_sdk::Chunk::new_null(
                    &format!("com.nominal.cafe-agent-runtime/{}", agent_id2),
                );
                for (k, v) in annotations {
                    chunk = chunk.with_annotation(k, v);
                }
                if let Err(e) = client.publish(&sid2, chunk).await {
                    warn!(
                        "cafe-agent-runtime: failed to publish initial chunk for session {}: {}",
                        sid2, e
                    );
                }
            });
        }

        let executor = Arc::new(PipelineExecutor::new(
            pipeline_info.steps.clone(),
            Duration::from_secs(pipeline_info.rpc_timeout_secs),
            pipeline_info.max_pipeline_depth,
        ));
        tokio::spawn(async move {
            session_loop::run_session_loop(sid.clone(), sp, executor).await;
        });
    }
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


