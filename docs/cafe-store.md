# cafe-store — Build Guide

**Role:** Persistence service. Subscribes to all session activity on the bus and
writes chunks + session metadata to SQLite. Also serves history to cafe-bus on request.

**Build after:** `cafe-types`, `cafe-bus`

---

## What it does

- Connects to cafe-bus with `subscribe_all`
- Persists every chunk to SQLite as it arrives
- Persists session create/delete events
- Exposes a secondary Unix socket for direct history queries by cafe-bus
  (so the bus can replay history to new subscribers without holding it all in memory)

> **v0.1 simplification:** The bus holds history in memory and doesn't call cafe-store
> for replay. cafe-store is purely a durable backup. In v0.2, the bus can evict old
> history and delegate to cafe-store.

---

## Cargo.toml dependencies to add

```toml
[dependencies]
cafe-types  = { path = "../cafe-types" }
tokio       = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow      = { workspace = true }
sqlx        = { version = "0.7", features = ["sqlite", "runtime-tokio", "json", "migrate"] }
```

---

## File structure

```
cafe-store/src/
```

---

## SQLite schema

```sql
-- sessions table
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    is_background INTEGER NOT NULL DEFAULT 0,
    display_name TEXT,
    ui_mode     TEXT NOT NULL DEFAULT 'chat',
    created_at  INTEGER NOT NULL,  -- Unix ms
    updated_at  INTEGER NOT NULL
);

-- chunks table
CREATE TABLE IF NOT EXISTS chunks (
    id           TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    content_type TEXT NOT NULL,           -- 'text', 'binary', 'null'
    content      TEXT,                    -- NULL for binary/null chunks
    data         BLOB,                    -- NULL for text/null chunks
    mime_type    TEXT,
    producer     TEXT NOT NULL,
    annotations  TEXT NOT NULL,           -- JSON string
    timestamp    INTEGER NOT NULL,        -- Unix ms
    seq          INTEGER NOT NULL,        -- insertion order within session
    UNIQUE(session_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_chunks_session ON chunks(session_id, seq);
```

---

## db.rs — key functions to implement

```rust
pub struct Db {
    pool: sqlx::SqlitePool,
}

impl Db {
    pub async fn connect(path: &str) -> anyhow::Result<Self>;
    pub async fn migrate(&self) -> anyhow::Result<()>;

    pub async fn upsert_session(&self, session_id: &str, agent_id: &str, ...) -> anyhow::Result<()>;
    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()>;
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<SessionRow>>;

    pub async fn insert_chunk(&self, session_id: &str, chunk: &Chunk) -> anyhow::Result<()>;
    pub async fn load_history(&self, session_id: &str) -> anyhow::Result<Vec<Chunk>>;
}
```

Store `annotations` as a JSON string (`serde_json::to_string`).
Store `data` (binary) directly as BLOB — do not base64 in the database.

---

## Reconnect logic

cafe-store must handle cafe-bus restarts gracefully:

```rust
loop {
    match connect_and_run(&config, &db).await {
        Ok(()) => break,  // clean shutdown
        Err(e) => {
            tracing::warn!("bus connection lost: {}. Reconnecting in 2s", e);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}
```

On reconnect, send `subscribe_all` again. Duplicate chunks (same `id`) should be
ignored with `INSERT OR IGNORE`.

---

## Session scan on startup

On startup before connecting to the bus, cafe-store should log the count of
sessions and chunks in the database. This gives a health check baseline.

---

## Environment variables

| Variable        | Default      | Description              |
|-----------------|--------------|--------------------------|
| `CAFE_BUS_SOCKET` | `/tmp/cafe-bus.sock` | Bus socket path |
| `CAFE_DB_PATH`  | `./cafe.db`  | SQLite file path         |
