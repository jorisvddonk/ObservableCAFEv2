mod config;
mod executor;
mod lifecycle;
mod loader;
mod registry;
mod scheduler;
mod schema_registry;
mod session_loop;
mod tool_detector;
mod tool_executor;
mod watcher;

use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{AgentDefinition, ServerMessage, StepDef};
use config::Config;
use executor::PipelineExecutor;
use registry::{AgentEntry, AgentRegistry};
use schema_registry::SchemaRegistry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tracing::{error, info, warn};

/// Names of agents defined in the JS agent directories (`./agents-js` plus
/// `CAFE_JS_AGENT_PATHS`). Scanned without executing anything — only the
/// manifest is read.
fn scan_js_agent_names() -> HashSet<String> {
    let mut names = HashSet::new();
    for dir in cafe_js_manifest::agent_dirs() {
        for loaded in cafe_js_manifest::scan_directory(&dir).loaded {
            names.insert(loaded.manifest.name);
        }
    }
    names
}

/// Drop TOML agents whose name is claimed by a JS agent (JS wins).
/// Returns the removed names.
fn remove_shadowed(
    agents: &mut Vec<(PathBuf, AgentDefinition)>,
    js_names: &HashSet<String>,
) -> Vec<String> {
    let mut removed = Vec::new();
    agents.retain(|(_, def)| {
        if js_names.contains(&def.name) {
            removed.push(def.name.clone());
            false
        } else {
            true
        }
    });
    // A name can appear in several TOML dirs; report each shadowed name once.
    removed.sort();
    removed.dedup();
    removed
}

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

    // 1b. JS agents shadow TOML agents of the same name (ADR-127). Without
    // this, both cafe-agent-runtime and cafe-agent-js attach to the same
    // session and every step runs twice.
    let js_names = scan_js_agent_names();
    let shadowed = remove_shadowed(&mut all_agents, &js_names);
    if !shadowed.is_empty() {
        info!(
            "cafe-agent-runtime: {} agent(s) shadowed by JS agents: {:?}",
            shadowed.len(),
            shadowed
        );
    }

    // 2. Wait for bus to be ready
    if let Err(e) = cafe_sdk::bus::wait_for_bus(&config.socket_path, Duration::from_millis(500), 60).await {
        warn!("cafe-agent-runtime: bus not ready after 30s, continuing anyway: {e}");
    }

    // 2b. Start schema discovery (subscribe to __schema__ session)
    let schema_registry = SchemaRegistry::new();
    let _schema_task = schema_registry
        .clone()
        .start_discovery(config.socket_path.clone())
        .await;
    // Brief wait for existing evaluators to announce their schemas
    tokio::time::sleep(Duration::from_millis(500)).await;

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
        let sreg = schema_registry.clone();
        tokio::spawn(async move {
            run_pipeline_subscriber(sp, pipelines, sreg).await;
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
    schema_registry: SchemaRegistry,
) {
    let client = cafe_sdk::bus::BusClient::unix(&socket_path);
    let mut rx = match client.subscribe_all().await {
        Ok(rx) => rx,
        Err(e) => {
            warn!("cafe-agent-runtime: subscribe_all failed: {}", e);
            return;
        }
    };

    // Sessions that already have a session_loop attached. Prevents duplicate
    // pipelines: subscribe_all replays existing sessions as SessionCreated events,
    // which would otherwise double-attach the same session (startup re-attach +
    // SessionCreated replay).
    let attached: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    // Re-attach to existing sessions on startup so they aren't orphaned after restart
    if let Ok(sessions) = client.list_sessions().await {
        for session in &sessions {
            if let Some(pipeline_info) = agent_pipelines.get(&session.agent_id) {
                info!(
                    "cafe-agent-runtime: re-attaching pipeline to existing session {} (agent {})",
                    session.session_id, session.agent_id
                );
                attach_to_session(
                    &socket_path,
                    &session.session_id,
                    &session.agent_id,
                    pipeline_info,
                    &schema_registry,
                    client.clone(),
                    attached.clone(),
                )
                .await;
            }
        }
    }

    while let Some(msg) = rx.recv().await {
        match msg {
            ServerMessage::SessionCreated { session_id, agent_id } => {
                let pipeline_info = match agent_pipelines.get(&agent_id) {
                    Some(info) => info,
                    None => {
                        info!(
                            "cafe-agent-runtime: ignoring session {} (agent {} not in pipeline map)",
                            session_id, agent_id
                        );
                        continue;
                    }
                };

                info!(
                    "cafe-agent-runtime: attaching pipeline to session {} (agent {})",
                    session_id, agent_id
                );

                attach_to_session(
                    &socket_path,
                    &session_id,
                    &agent_id,
                    pipeline_info,
                    &schema_registry,
                    client.clone(),
                    attached.clone(),
                )
                .await;
            }
            ServerMessage::SessionDeleted { session_id } => {
                // Forget the session so a future SessionCreated (session recreated
                // with the same id) can attach a fresh pipeline.
                attached.lock().unwrap_or_else(PoisonError::into_inner).remove(&session_id);
                info!(
                    "cafe-agent-runtime: cleared pipeline attachment for deleted session {}",
                    session_id
                );
            }
            _ => {}
        }
    }
}

/// Publish initial config chunk + evaluator schemas into a session, then spawn
/// a session_loop task to watch for trigger chunks and execute the pipeline.
async fn attach_to_session(
    socket_path: &str,
    session_id: &str,
    agent_id: &str,
    pipeline_info: &AgentPipelineInfo,
    schema_registry: &SchemaRegistry,
    client: BusClient,
    attached: Arc<Mutex<HashSet<String>>>,
) {
    let sid = session_id.to_string();
    let sp = socket_path.to_string();
    let agent_id2 = agent_id.to_string();

    // Register the session as attached. If a pipeline is already running for it
    // (e.g. startup re-attach racing the SessionCreated replay from subscribe_all),
    // skip to avoid duplicate session_loop tasks and duplicated RPC dispatches.
    {
        let mut set = attached.lock().unwrap_or_else(PoisonError::into_inner);
        if !set.insert(sid.clone()) {
            info!(
                "cafe-agent-runtime: skipping duplicate pipeline for session {} (agent {})",
                sid, agent_id
            );
            return;
        }
    }

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

    // Publish schema chunks for each evaluator used by this agent
    {
        let sreg = schema_registry.clone();
        let client = client.clone();
        let sid2 = sid.clone();
        let steps = pipeline_info.steps.clone();
        tokio::spawn(async move {
            for step in &steps {
                // Only publish schemas for RPC evaluators (not built-in)
                let is_builtin = matches!(
                    step.step_type.as_str(),
                    "role-annotator" | "trust-filter" | "tool-detector" | "tool-executor" | "mcp"
                );
                if is_builtin {
                    continue;
                }
                if let Some(schema) = sreg.get(&step.step_type).await {
                    let chunk = cafe_sdk::Chunk::new_null("com.nominal.cafe-agent-runtime")
                        .with_annotation(cafe_sdk::keys::CAFE_SCHEMA_EVALUATOR, &schema);
                    if let Err(e) = client.publish(&sid2, chunk).await {
                        warn!(
                            "cafe-agent-runtime: failed to publish schema for '{}': {}",
                            schema.name, e
                        );
                    }
                }
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


#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str) -> (PathBuf, AgentDefinition) {
        (
            PathBuf::from(format!("{name}.toml")),
            AgentDefinition {
                name: name.into(),
                ..AgentDefinition::default()
            },
        )
    }

    #[test]
    fn remove_shadowed_drops_js_claimed_names() {
        let mut agents = vec![def("default"), def("dice"), def("rot13")];
        let js: HashSet<String> = ["default".to_string(), "rot13".to_string()]
            .into_iter()
            .collect();
        let removed = remove_shadowed(&mut agents, &js);
        assert_eq!(removed, vec!["default".to_string(), "rot13".to_string()]);
        let names: Vec<&str> = agents.iter().map(|(_, d)| d.name.as_str()).collect();
        assert_eq!(names, vec!["dice"]);
    }

    #[test]
    fn remove_shadowed_reports_each_name_once() {
        // Same agent defined in two TOML dirs (e.g. repo + private).
        let mut agents = vec![def("default"), def("default")];
        let js: HashSet<String> = ["default".to_string()].into_iter().collect();
        assert_eq!(remove_shadowed(&mut agents, &js), vec!["default".to_string()]);
        assert!(agents.is_empty());
    }

    #[test]
    fn remove_shadowed_no_js_agents_is_a_noop() {
        let mut agents = vec![def("default")];
        let removed = remove_shadowed(&mut agents, &HashSet::new());
        assert!(removed.is_empty());
        assert_eq!(agents.len(), 1);
    }
}
