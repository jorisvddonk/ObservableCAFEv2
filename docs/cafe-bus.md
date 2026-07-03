# cafe-bus — Build Guide

**Role:** Central message broker. Owns session state. Routes chunks between all services.
All other services connect to cafe-bus; they do not talk to each other directly.

**Build after:** `cafe-types`

---

## What it does

- Listens on a Unix domain socket
- Maintains a registry of sessions (`session_id → SessionState`)
- When a chunk is published to a session, broadcasts it to all subscribers
- Replays session history to new subscribers
- Accepts session create/delete commands
- Does NOT call LLM backends, does NOT persist to disk (that's cafe-store)

---

## Cargo.toml dependencies to add

```toml
[dependencies]
cafe-types    = { path = "../cafe-types" }
tokio         = { workspace = true }
serde         = { workspace = true }
serde_json    = { workspace = true }
tracing       = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow        = { workspace = true }
```

---

## File structure

```
cafe-bus/src/
```

---

## Core data structures

```rust
// registry.rs
pub struct SessionRegistry {
    sessions: HashMap<String, SessionState>,
}

// session.rs
pub struct SessionState {
    pub session_id: String,
    pub agent_id: String,
    pub history: Vec<Chunk>,
    pub tx: broadcast::Sender<Chunk>,  // capacity: 1024
}

impl SessionState {
    pub fn new(session_id: String, agent_id: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { session_id, agent_id, history: Vec::new(), tx }
    }

    pub fn publish(&mut self, chunk: Chunk) {
        self.history.push(chunk.clone());
        // ignore send errors (no active subscribers is fine)
        let _ = self.tx.send(chunk);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Chunk> {
        self.tx.subscribe()
    }
}
```

---

## Client handler pseudocode

```rust
async fn handle_client(stream: UnixStream, registry: Arc<RwLock<SessionRegistry>>) {
    let (reader, writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let writer = Arc::new(Mutex::new(writer));

    while let Some(line) = lines.next_line().await? {
        let msg: ClientMessage = serde_json::from_str(&line)?;
        match msg {
            ClientMessage::Publish { session_id, chunk } => {
                let mut reg = registry.write().await;
                if let Some(session) = reg.get_mut(&session_id) {
                    session.publish(chunk);
                }
            }
            ClientMessage::Subscribe { session_id } => {
                // 1. Get history snapshot + subscribe receiver (holding only read lock)
                // 2. Send history chunks to client
                // 3. Send HistoryComplete event
                // 4. Forward live chunks from receiver in a spawned task
            }
            ClientMessage::CreateSession { session_id, agent_id, config } => {
                // Insert into registry, emit SessionCreated, emit config as null chunk
            }
            // ... etc
        }
    }
}
```

Key: when subscribing, hold the registry lock only long enough to clone the history
and get the receiver. Never hold a lock across an await.

---

## Startup sequence in main.rs

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    // Remove stale socket if it exists
    let _ = std::fs::remove_file(&config.socket_path);

    let listener = UnixListener::bind(&config.socket_path)?;
    // Set permissions so only owner can connect
    std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o600))?;

    let registry = Arc::new(RwLock::new(SessionRegistry::new()));

    tracing::info!("cafe-bus listening on {}", config.socket_path);

    // Handle SIGTERM gracefully
    let shutdown = setup_shutdown_signal();

    loop {
        tokio::select! {
            Ok((stream, _)) = listener.accept() => {
                tokio::spawn(handle_client(stream, registry.clone()));
            }
            _ = shutdown.notified() => {
                tracing::info!("cafe-bus shutting down");
                break;
            }
        }
    }
    Ok(())
}
```

---

## Implementation notes

- Use `tokio::sync::broadcast` for fan-out to multiple subscribers. Capacity 1024.
- If a subscriber's receiver falls behind (lagged), send an error event and close
  that subscription. The client can re-subscribe to get a fresh replay.
- Session history grows unboundedly in memory. This is acceptable for v0.1.
  Future: cap history at N chunks and rely on cafe-store for full replay.
- The bus trusts all connected clients equally. Authorization happens in cafe-server.
- Write a health check: after binding, touch a file at `$CAFE_BUS_SOCKET.ready`.
  process-compose probes for this file.

---

## Testing

Integration test: spawn cafe-bus in a child process, connect two clients,
publish a chunk from one, assert the other receives it.

```rust
// tests/integration.rs
#[tokio::test]
async fn test_publish_subscribe() {
    // start bus, connect two clients, subscribe one, publish from other, assert receipt
}
```
