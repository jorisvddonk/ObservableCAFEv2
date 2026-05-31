# cafe-agent-runtime — Build Guide

**Role:** Agent host. Discovers agent definitions from disk, registers them with the
bus, manages their lifecycle (init, hot-reload, destroy), and runs scheduled tasks
for background agents.

**Build after:** `cafe-types`, `cafe-bus`

---

## What it does

- Scans configured directories for agent definition files (TOML)
- Registers each agent with the bus (creates a background session for `background: true` agents)
- Monitors agent files for changes and hot-reloads changed agents
- Runs cron schedules defined by background agents
- Exposes an internal API for cafe-server's `/api/admin/agents` endpoint (via bus signals)

---

## Agent definition format (TOML)

Agents are defined as TOML files. The agent runtime reads these; it does not execute
arbitrary code (unlike the original TypeScript version which used `eval`).

```toml
# agents/default.toml
name = "default"
description = "Standard chat agent"
background = false
allows_reload = true
persists_state = true

# Which evaluators to chain, in order
pipeline = ["trust-filter", "llm"]

# Optional: cron schedule (background agents only)
# schedule = "0 7 * * *"

# Optional: config schema (JSON Schema subset)
[config_schema]
type = "object"
properties = { model = { type = "string" } }
```

```toml
# agents/rss-summarizer.toml
name = "rss-summarizer"
description = "Fetches and summarizes RSS feeds daily"
background = true
allows_reload = true
persists_state = false
schedule = "0 7 * * *"    # 07:00 daily

pipeline = ["rss-fetch", "llm-summarize"]

[config]
rss_url = "https://news.ycombinator.com/rss"
```

---

## Built-in evaluator types

The pipeline field references evaluator types by name. cafe-agent-runtime ships
with these built-in evaluators:

| Name            | Description                                              |
|-----------------|----------------------------------------------------------|
| `llm`           | Calls cafe-llm (via bus) with session history            |
| `trust-filter`  | Drops untrusted chunks before they reach the LLM         |
| `role-annotator`| Sets `chat.role: user` if not already set                |
| `rss-fetch`     | Fetches RSS feed URL, emits one chunk per item           |
| `tool-detector` | Scans LLM output for `<|tool_call|>` syntax, dispatches |

Future evaluators can be added as TOML-declarable plugins.

---

## Cargo.toml dependencies to add

```toml
[dependencies]
cafe-types           = { path = "../cafe-types" }
tokio                = { workspace = true }
serde                = { workspace = true }
serde_json           = { workspace = true }
tracing              = { workspace = true }
tracing-subscriber   = { workspace = true }
anyhow               = { workspace = true }
toml                 = "0.8"
notify               = "6"           # file watching
tokio-cron-scheduler = "0.10"
sha2                 = "0.10"        # file hashing for change detection
glob                 = "0.3"
```

---

## File structure

```
cafe-agent-runtime/src/
├── main.rs             # scan dirs, start background agents, run watcher loop
├── loader.rs           # scan directories, parse TOML agent definitions
├── registry.rs         # AgentRegistry: name → AgentDef + session_id
├── lifecycle.rs        # create/destroy/reload agent sessions via bus
├── scheduler.rs        # cron scheduling for background agents
├── evaluators/
│   ├── mod.rs          # Evaluator enum + dispatch
│   ├── trust_filter.rs
│   ├── role_annotator.rs
│   └── rss_fetch.rs
├── watcher.rs          # notify-based file watcher → reload events
└── config.rs           # Config from env
```

---

## Startup sequence

```
1. Read CAFE_AGENT_PATHS (colon-separated directories)
2. Scan each directory for *.toml files
3. Parse each file into AgentDefinition
4. Connect to cafe-bus
5. For each agent with background = true:
   a. Send CreateSession { session_id: agent.name, agent_id: agent.name }
   b. Subscribe to that session
   c. Start cron schedule if defined
6. Begin file watcher loop
7. Emit "agents ready" log line
```

---

## Hot reload logic

```rust
// When a file change is detected:
async fn reload_agent(name: &str, registry: &mut AgentRegistry, bus: &BusClient) {
    let new_def = load_agent_file(path)?;

    // Hash comparison
    let old_hash = registry.get_hash(name);
    let new_hash = hash_file(path);
    if old_hash == new_hash { return; }  // no change

    let agent = registry.get(name).unwrap();
    if !agent.def.allows_reload { return; }

    // Destroy old session's pipeline (send flow.signal: reset to session)
    // Re-init with new definition
    // Update registry
}
```

---

## Background agent scheduling

```rust
let scheduler = JobScheduler::new().await?;

if let Some(cron_expr) = &agent.schedule {
    let bus = bus.clone();
    let session_id = agent.name.clone();
    scheduler.add(
        Job::new_async(cron_expr, move |_, _| {
            let bus = bus.clone();
            let session_id = session_id.clone();
            Box::pin(async move {
                // Publish a null trigger chunk to the agent's session
                let trigger = Chunk::new_null("com.nominal.scheduler")
                    .with_annotation("flow.signal", "tick");
                bus.publish(&session_id, trigger).await.ok();
            })
        })?
    ).await?;
}

scheduler.start().await?;
```

---

## Environment variables

| Variable              | Default                    | Description                          |
|-----------------------|----------------------------|--------------------------------------|
| `CAFE_BUS_SOCKET`     | `/tmp/cafe-bus.sock`       | Bus socket path                      |
| `CAFE_AGENT_PATHS`    | `./agents`                 | Colon-separated agent search paths   |
