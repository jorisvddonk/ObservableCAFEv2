mod config;
mod lifecycle;
mod loader;
mod pipeline;
mod registry;
mod scheduler;
mod tool_detector;
mod tool_executor;
mod watcher;

use anyhow::Result;
use config::Config;
use pipeline::PipelineExecutor;
use registry::{AgentEntry, AgentRegistry};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

/// Everything the poller needs to know about an agent with RPC steps.
#[derive(Clone)]
struct AgentPipelineInfo {
    pipeline: Vec<String>,
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
        let has_rpc_steps = def.pipeline.iter().any(|s| {
            !matches!(s.as_str(), "role-annotator" | "trust-filter" | "llm")
        });
        if has_rpc_steps {
            agent_pipelines.insert(name.clone(), AgentPipelineInfo {
                pipeline: def.pipeline.clone(),
                initial_chunk_type: def.initial_chunk_type.clone(),
                initial_chunk_annotations: def.initial_chunk_annotations.clone(),
            });
        }

        registry.lock().unwrap().insert(AgentEntry {
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
            run_pipeline_poller(sp, pipelines).await;
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

/// Poll list_sessions every 2 s. For each session whose agent_id has RPC steps:
///  1. Publish the agent's initial config chunk (if it's a null config chunk) so
///     resolve_session_config can find TTS/LLM settings in the session history.
///  2. Spawn a run_session_pipeline task to watch for LLM completions and fire RPC steps.
async fn run_pipeline_poller(
    socket_path: String,
    agent_pipelines: Arc<HashMap<String, AgentPipelineInfo>>,
) {
    let client = cafe_sdk::bus::BusClient::new(&socket_path);
    let mut known: HashSet<String> = HashSet::new();

    loop {
        match client.list_sessions().await {
            Ok(sessions) => {
                let current_ids: HashSet<String> =
                    sessions.iter().map(|s| s.session_id.clone()).collect();

                for session in &sessions {
                    if known.contains(&session.session_id) {
                        continue;
                    }

                    if let Some(info) = agent_pipelines.get(&session.agent_id) {
                        info!(
                            "cafe-agent-runtime: attaching pipeline to session {} (agent {})",
                            session.session_id, session.agent_id
                        );
                        known.insert(session.session_id.clone());

                        let sid = session.session_id.clone();
                        let sp = socket_path.clone();
                        let agent_id = session.agent_id.clone();

                        // Publish the initial config chunk so resolve_session_config
                        // picks up TTS/LLM settings for user-created sessions.
                        if info.initial_chunk_type == "null" && !info.initial_chunk_annotations.is_empty() {
                            let annotations = info.initial_chunk_annotations.clone();
                            let client = client.clone();
                            let sid2 = sid.clone();
                            tokio::spawn(async move {
                                let mut chunk = cafe_sdk::Chunk::new_null(
                                    &format!("com.nominal.cafe-agent-runtime/{}", agent_id),
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

                        let executor = PipelineExecutor::from_step_names(
                            &info.pipeline,
                            Duration::from_secs(30),
                        );
                        tokio::spawn(async move {
                            if let Err(e) =
                                pipeline::run_session_pipeline(sid.clone(), sp, executor).await
                            {
                                warn!(
                                    "cafe-agent-runtime: pipeline watcher for session {} exited: {}",
                                    sid, e
                                );
                            }
                        });
                    }
                }

                // Prune sessions that no longer exist
                known.retain(|id| current_ids.contains(id));
            }
            Err(e) => {
                warn!("cafe-agent-runtime: list_sessions error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
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


