mod bridge;
mod loader;
mod scheduler;
mod watcher;

use anyhow::Result;
use bridge::{
    assemble_llm_text, classify_event, fetch_config_json, run_js_source, run_js_stream,
    ConfigSlot, EventRx, JsEvent,
};
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, ServerMessage, SessionConfig};
use loader::{JsAgent, Registry};
use scheduler::JsScheduler;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Fallback per-agent RPC timeout when the manifest sets none.
const DEFAULT_RPC_TIMEOUT_SECS: u64 = 30;

struct Config {
    socket_path: String,
    agent_dirs: Vec<String>,
    default_rpc_timeout: Duration,
}

impl Config {
    fn from_env() -> Self {
        Self {
            socket_path: std::env::var("CAFE_BUS_SOCKET")
                .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into()),
            agent_dirs: cafe_js_manifest::agent_dirs(),
            default_rpc_timeout: std::env::var("CAFE_JS_RPC_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(DEFAULT_RPC_TIMEOUT_SECS)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let config = Config::from_env();

    // QuickJS runtimes are single-threaded (!Send), so all session tasks live
    // on one LocalSet thread; tokio::spawn_local does not require Send. The
    // reconnect loop is local too — `run_with_reconnect` needs Send futures.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut delay = Duration::from_secs(1);
            loop {
                match run_once(
                    &config.socket_path,
                    &config.agent_dirs,
                    config.default_rpc_timeout,
                )
                .await
                {
                    Ok(()) => {
                        info!("cafe-agent-js: clean shutdown");
                        return;
                    }
                    Err(e) => {
                        warn!("cafe-agent-js: error (retrying in {delay:?}): {e}");
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                }
            }
        })
        .await;

    Ok(())
}

/// One connected pass: load registry, start background sessions + scheduler,
/// re-attach existing sessions, then serve bus + watcher events until the
/// connection drops (reconnect re-runs everything).
async fn run_once(socket_path: &str, dirs: &[String], default_timeout: Duration) -> Result<()> {
    let client = BusClient::unix(socket_path);
    let (registry, warnings) = loader::load_all(dirs);
    {
        let reg = registry.read().unwrap_or_else(PoisonError::into_inner);
        info!("cafe-agent-js: loaded {} JS agents from {dirs:?}", reg.len());
    }
    for w in warnings {
        warn!("cafe-agent-js: {w}");
    }

    // Background agents: session + config seeding + cron. Snapshot first —
    // guards must not be held across awaits.
    let sched = JsScheduler::new().await?;
    let background: Vec<JsAgent> = {
        let reg = registry.read().unwrap_or_else(PoisonError::into_inner);
        let mut background: Vec<JsAgent> = reg
            .values()
            .filter(|a| a.manifest.background)
            .cloned()
            .collect();
        background.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        background
    };
    for agent in &background {
        if let Err(e) = ensure_background_session(&client, agent).await {
            warn!(
                "cafe-agent-js: background agent '{}' failed: {e}",
                agent.manifest.name
            );
            continue;
        }
        if let Some(cron) = agent.manifest.schedule.clone() {
            if let Err(e) = sched
                .schedule(
                    agent.manifest.name.clone(),
                    &cron,
                    socket_path.to_string(),
                )
                .await
            {
                warn!(
                    "cafe-agent-js: invalid schedule '{cron}' for '{}': {e}",
                    agent.manifest.name
                );
            }
        }
    }
    sched.start().await?;

    let attached: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let stateful: StatefulMap = Arc::new(Mutex::new(HashMap::new()));

    // Re-attach to existing sessions so restarts don't orphan them.
    if let Ok(sessions) = client.list_sessions().await {
        for session in &sessions {
            if registry
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&session.agent_id)
            {
                attach(
                    socket_path,
                    session.session_id.clone(),
                    session.agent_id.clone(),
                    registry.clone(),
                    attached.clone(),
                    stateful.clone(),
                    default_timeout,
                )
                .await;
            }
        }
    }

    // Hot-reload watcher (best-effort: changes apply to subsequently read
    // sources; schedule changes log that a restart is required).
    let (_watcher_handle, mut change_rx) = match watcher::start_watcher(dirs) {
        Ok(w) => (Some(w.0), Some(w.1)),
        Err(e) => {
            warn!("cafe-agent-js: file watcher failed to start: {e}");
            (None, None)
        }
    };

    let mut rx = client.subscribe_all().await?;
    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                match msg {
                    ServerMessage::SessionCreated { session_id, agent_id } => {
                        if registry.read().unwrap_or_else(PoisonError::into_inner).contains_key(&agent_id) {
                            info!("cafe-agent-js: attaching to session {session_id} (agent {agent_id})");
                            attach(socket_path, session_id, agent_id, registry.clone(), attached.clone(), stateful.clone(), default_timeout).await;
                        }
                    }
                    ServerMessage::SessionDeleted { session_id } => {
                        attached.lock().unwrap_or_else(PoisonError::into_inner).remove(&session_id);
                        // Dropping the entry closes the event stream; abort
                        // the task too so a wedged runtime dies promptly.
                        if let Some(entry) = stateful.lock().unwrap_or_else(PoisonError::into_inner).remove(&session_id) {
                            if let Some(task) = entry.task {
                                task.abort();
                            }
                            info!("cafe-agent-js: tore down stateful runtime for deleted session {session_id}");
                        }
                    }
                    _ => {}
                }
            }
            change = async {
                match change_rx.as_mut() {
                    Some(ch) => ch.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(path) = change {
                    // Watcher also reports removals: a path that no longer
                    // exists unloads its agent instead of failing to parse.
                    if path.exists() {
                        match loader::reload_file(&registry, &path) {
                            Ok(name) => info!("cafe-agent-js: hot-reloaded agent '{name}' ({})", path.display()),
                            Err(e) => warn!("cafe-agent-js: reload of {} ignored: {e:#}", path.display()),
                        }
                    } else {
                        match loader::remove_by_path(&registry, &path) {
                            Some(name) => info!(
                                "cafe-agent-js: unloaded agent '{name}' ({} removed); its sessions stop on next event",
                                path.display()
                            ),
                            None => {} // file outside any agent dir / never loaded
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn attach(
    socket_path: &str,
    session_id: String,
    agent_id: String,
    registry: Registry,
    attached: Arc<Mutex<HashSet<String>>>,
    stateful: StatefulMap,
    default_timeout: Duration,
) {
    {
        let mut set = attached.lock().unwrap_or_else(PoisonError::into_inner);
        if !set.insert(session_id.clone()) {
            return;
        }
    }
    // Seed the manifest's initial config so user-created sessions resolve
    // LLM/system settings (parity with the legacy runtime's attach publish).
    // Later-wins merging makes re-attach duplicates harmless.
    if let Some(agent) = registry
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&agent_id)
    {
        if !agent.manifest.initial_config.is_empty() {
            let client = BusClient::unix(socket_path);
            publish_initial_config(&client, &session_id, &agent.manifest.name, &agent.manifest.initial_config).await;
        }
    }
    let sp = socket_path.to_string();
    tokio::task::spawn_local(async move {
        if let Err(e) =
            run_session(&sp, &session_id, &agent_id, registry, stateful, default_timeout).await
        {
            warn!("cafe-agent-js: session {session_id} error: {e}");
        }
    });
}

/// Publish the manifest's initial config as a runtime null chunk into
/// `session_id`. `config.type = "runtime"` is injected when absent.
async fn publish_initial_config(
    client: &BusClient,
    session_id: &str,
    agent_name: &str,
    initial_config: &std::collections::HashMap<String, serde_json::Value>,
) {
    let mut chunk = Chunk::new_null(format!("com.nominal.cafe-agent-js/{agent_name}"));
    let mut annotations = initial_config.clone();
    annotations
        .entry(keys::CONFIG_TYPE.into())
        .or_insert_with(|| "runtime".into());
    for (key, value) in &annotations {
        chunk = chunk.with_annotation(key, value);
    }
    #[allow(deprecated)]
    if let Err(e) = client.publish(session_id, chunk).await {
        warn!("cafe-agent-js: failed to seed config for {session_id}: {e}");
    } else {
        info!("cafe-agent-js: seeded config for {session_id} (agent {agent_name})");
    }
}

// ---------------------------------------------------------------------------
// Stateful session runtimes
// ---------------------------------------------------------------------------

/// Bookkeeping for one stateful session's long-lived JS runtime.
/// The channel and config slot outlive any single task so a respawned
/// runtime (hot-reload, main-returned restart) resumes the same stream.
struct StatefulEntry {
    tx: UnboundedSender<String>,
    rx: EventRx,
    config: ConfigSlot,
    /// Hash of the agent source the current task runs. Compared per event;
    /// a mismatch (hot-reload) respawns the task on the shared channel.
    source_hash: String,
    task: Option<JoinHandle<()>>,
}

type StatefulMap = Arc<Mutex<HashMap<String, StatefulEntry>>>;

/// Publish a JS error chunk without holding a subscription (fire-and-forget;
/// the one-shot `publish` deprecation does not apply — no reply is expected).
async fn publish_error_chunk(client: &BusClient, session_id: &str, message: String) {
    let err_chunk = Chunk::new_null("com.nominal.cafe-agent-js")
        .with_annotation(keys::CAFE_ERROR_MESSAGE, message)
        .with_annotation("error.source", "js-agent");
    #[allow(deprecated)]
    if let Err(e) = client.publish(session_id, err_chunk).await {
        warn!("cafe-agent-js: failed to publish error chunk for {session_id}: {e}");
    }
}

/// Spawn (or respawn) the long-lived JS task for a stateful session.
/// The task ends when `main(cafe)` settles: normally via channel close on
/// session teardown, or via a JS throw — both publish an error chunk only
/// in the failure case.
fn spawn_stateful_task(
    client: &BusClient,
    session_id: &str,
    agent_id: &str,
    source: String,
    timeout: Duration,
    rx: EventRx,
    config: ConfigSlot,
) -> JoinHandle<()> {
    let client = client.clone();
    let session_id = session_id.to_string();
    let agent_id = agent_id.to_string();
    tokio::task::spawn_local(async move {
        match run_js_stream(
            &client,
            &session_id,
            &agent_id,
            &source,
            timeout,
            rx,
            config,
        )
        .await
        {
            Ok(ret) => info!(
                "cafe-agent-js: stateful main() for {session_id} returned ({ret}); restarts on next event"
            ),
            Err(e) => {
                warn!("cafe-agent-js: stateful agent error in {session_id}: {e}");
                publish_error_chunk(&client, &session_id, e.to_string()).await;
            }
        }
    })
}

/// Create a background session idempotently and seed its config chunk.
/// `SESSION_EXISTS` (restart against a live bus) reuses the session.
async fn ensure_background_session(client: &BusClient, agent: &JsAgent) -> Result<()> {
    let name = &agent.manifest.name;
    match client
        .create_session(name, name, SessionConfig::default())
        .await
    {
        Ok(()) => info!("cafe-agent-js: created background session '{name}'"),
        Err(e) if e.code() == Some("SESSION_EXISTS") => {
            info!("cafe-agent-js: background session '{name}' already exists, reusing");
        }
        Err(e) => anyhow::bail!("create_session failed: {e}"),
    }

    if !agent.manifest.initial_config.is_empty() {
        publish_initial_config(client, name, name, &agent.manifest.initial_config).await;
    }
    Ok(())
}

/// Per-session loop: history-gated (ADR-123), transient-skipping.
///
/// Stateless agents run one fresh runtime per event (source re-read from the
/// registry each time, so hot-reload applies without re-attach). Stateful
/// agents feed a shared channel into one long-lived runtime, respawned when
/// its source hash changes (hot-reload) or its task finished (previous
/// `main()` returned).
async fn run_session(
    socket_path: &str,
    session_id: &str,
    agent_id: &str,
    registry: Registry,
    stateful: StatefulMap,
    default_timeout: Duration,
) -> Result<()> {
    let client = BusClient::unix(socket_path);
    // Persistent connection so request and response share one bus connection.
    let mut sub = client.subscribe_session(session_id).await?;
    let mut history_complete = false;

    while let Some(msg) = sub.rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                continue;
            }
            _ => continue,
        };
        if !history_complete {
            continue;
        }
        let Some(mut event) = classify_event(&chunk) else {
            continue;
        };
        // The llm_complete trigger is content-less; assemble the text the
        // agent must parse (tool markers live there) from history.
        if event.event_type == "llm_complete" {
            if let Ok(history) = client.get_history(session_id).await {
                if let Some(text) = assemble_llm_text(&history) {
                    event.text = text;
                }
            }
        }
        let agent = match registry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(agent_id)
            .cloned()
        {
            Some(agent) => agent,
            None => {
                warn!("cafe-agent-js: agent '{agent_id}' removed from registry; ending {session_id}");
                return Ok(());
            }
        };
        let timeout = agent
            .manifest
            .rpc_timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(default_timeout);
        info!("cafe-agent-js: {session_id} event {}", event.event_type);
        if agent.manifest.mode == "stateful" {
            drive_stateful(
                &client,
                session_id,
                agent_id,
                &agent,
                &event,
                timeout,
                &stateful,
            )
            .await;
        } else if let Err(e) =
            run_js_source(&client, session_id, agent_id, &event, &agent.source, timeout).await
        {
            warn!("cafe-agent-js: agent error in {session_id}: {e}");
            let err_chunk = Chunk::new_null("com.nominal.cafe-agent-js")
                .with_annotation(keys::CAFE_ERROR_MESSAGE, e.to_string())
                .with_annotation("error.source", "js-agent");
            if let Err(pub_err) = sub.publish(err_chunk).await {
                warn!("cafe-agent-js: failed to publish error chunk: {pub_err}");
            }
        }
    }
    Ok(())
}

/// Route one event into a stateful session's runtime: get-or-create the
/// entry, refresh its config snapshot, respawn on source change or finished
/// task, then deliver the event.
async fn drive_stateful(
    client: &BusClient,
    session_id: &str,
    agent_id: &str,
    agent: &JsAgent,
    event: &JsEvent,
    timeout: Duration,
    stateful: &StatefulMap,
) {
    // Get-or-create the entry without holding the lock across awaits.
    let (tx, rx, config) = {
        let mut map = stateful.lock().unwrap_or_else(PoisonError::into_inner);
        match map.get(session_id) {
            Some(entry) => (entry.tx.clone(), entry.rx.clone(), entry.config.clone()),
            None => {
                let (tx, rx) = bridge::new_event_channel();
                let config = Arc::new(tokio::sync::Mutex::new(String::from("{}")));
                map.insert(
                    session_id.to_string(),
                    StatefulEntry {
                        tx: tx.clone(),
                        rx: rx.clone(),
                        config: config.clone(),
                        source_hash: String::new(),
                        task: None,
                    },
                );
                (tx, rx, config)
            }
        }
    };

    // Refresh the config snapshot this event will observe (same per-event
    // freshness as stateless runs).
    *config.lock().await = fetch_config_json(client, session_id).await;

    // Respawn when the source changed (hot-reload) or the previous task
    // finished (main returned). Lock only for the swap.
    let respawn = {
        let map = stateful.lock().unwrap_or_else(PoisonError::into_inner);
        match map.get(session_id) {
            Some(entry) => {
                entry.source_hash != agent.file_hash
                    || entry.task.as_ref().map(|t| t.is_finished()).unwrap_or(true)
            }
            None => true,
        }
    };
    if respawn {
        let mut map = stateful.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = map.get_mut(session_id) {
            if let Some(task) = entry.task.take() {
                task.abort();
            }
            entry.source_hash = agent.file_hash.clone();
            entry.task = Some(spawn_stateful_task(
                client,
                session_id,
                agent_id,
                agent.source.clone(),
                timeout,
                rx.clone(),
                config.clone(),
            ));
            info!("cafe-agent-js: (re)started stateful runtime for {session_id} (agent {agent_id})");
        }
    }

    let event_json = match serde_json::to_string(event) {
        Ok(s) => s,
        Err(e) => {
            warn!("cafe-agent-js: failed to serialize event: {e}");
            return;
        }
    };
    if tx.send(event_json).is_err() {
        warn!("cafe-agent-js: event stream for {session_id} is gone; dropping event");
    }
}
