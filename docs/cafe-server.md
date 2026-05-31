# cafe-server — Build Guide

**Role:** HTTP gateway. Translates REST + SSE HTTP requests into bus operations.
Handles authentication. No business logic — it is purely a protocol bridge.

**Build after:** `cafe-types`, `cafe-bus` (needs a running bus to be useful)

---

## What it does

- Exposes the HTTP API documented in `docs/spec-http-api.md`
- Validates bearer tokens
- Translates HTTP requests into `ClientMessage` ops on the bus
- Streams chunks back to HTTP clients as SSE
- Manages tokens and quickies in SQLite

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
axum          = { version = "0.7", features = ["multipart"] }
tower-http    = { version = "0.5", features = ["cors", "trace"] }
sqlx          = { version = "0.7", features = ["sqlite", "runtime-tokio"] }
tokio-stream  = "0.1"
uuid          = { version = "1", features = ["v4"] }
```

---

## File structure

```
cafe-server/src/
├── main.rs             # build router, bind port, run
├── router.rs           # axum Router definition: all routes + middleware
├── handlers/
│   ├── sessions.rs     # CRUD for sessions
│   ├── chat.rs         # POST /api/sessions/:id/chat (SSE streaming)
│   ├── stream.rs       # GET /api/sessions/:id/stream (persistent SSE)
│   ├── chunks.rs       # POST/PATCH/DELETE chunks
│   ├── quickies.rs     # CRUD for quickies
│   └── admin.rs        # token management, agent reload, status
├── auth.rs             # bearer token extractor middleware
├── bus_client.rs       # shared bus connection pool
├── db.rs               # SQLite for tokens + quickies
├── sse.rs              # SSE stream helpers
└── config.rs           # Config from env
```

---

## Authentication middleware (auth.rs)

Implement as an Axum extractor:

```rust
pub struct AuthUser {
    pub token_id: String,
    pub is_admin: bool,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser {
    // Extract "Authorization: Bearer <token>" header
    // Look up token in DB
    // Return 401 if missing/invalid
}

pub struct AdminUser(pub AuthUser);  // Rejects non-admin with 403
```

---

## Bus connection (bus_client.rs)

Maintain a single persistent connection to cafe-bus, shared across all request handlers
via `Arc<BusClient>` in Axum state.

```rust
pub struct BusClient {
    tx: mpsc::Sender<ClientMessage>,      // send to bus
    // subscriptions managed per-request
}

impl BusClient {
    /// Subscribe to a session and return a stream of chunks.
    /// Internally opens a new bus connection per subscription
    /// (each SSE stream needs its own subscriber).
    pub async fn subscribe(&self, session_id: &str) -> anyhow::Result<impl Stream<Item = Chunk>>;

    pub async fn publish(&self, session_id: &str, chunk: Chunk) -> anyhow::Result<()>;
    pub async fn create_session(&self, ...) -> anyhow::Result<()>;
    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()>;
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>>;
}
```

> Because each SSE stream needs its own bus subscription (its own broadcast receiver),
> open a fresh Unix socket connection per SSE stream. Connections are cheap.

---

## SSE streaming (sse.rs + handlers/chat.rs)

Axum SSE example:

```rust
use axum::response::sse::{Event, Sse};
use tokio_stream::StreamExt;

async fn chat_handler(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // 1. Publish user chunk to bus
    // 2. Subscribe to session output stream
    // 3. Map chunks to SSE events
    // 4. Terminate stream on chat.stream_complete null chunk

    let chunk_stream = state.bus.subscribe(&session_id).await.unwrap();

    let sse_stream = chunk_stream
        .map(|chunk| {
            let data = serde_json::to_string(&chunk).unwrap();
            Ok(Event::default().data(data))
        })
        .take_while(|result| {
            // Keep going until stream_complete
            if let Ok(event) = result {
                !is_stream_complete_event(event)
            } else {
                true
            }
        });

    Sse::new(sse_stream).keep_alive(KeepAlive::default())
}
```

---

## SQLite schema (db.rs)

```sql
CREATE TABLE IF NOT EXISTS tokens (
    id          TEXT PRIMARY KEY,
    token       TEXT NOT NULL UNIQUE,  -- the actual bearer value
    description TEXT,
    is_admin    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS quickies (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL,
    description     TEXT,
    emoji           TEXT,
    agent_id        TEXT NOT NULL,
    starter_message TEXT,
    config_json     TEXT,              -- JSON blob
    ui_mode         TEXT DEFAULT 'chat',
    display_order   INTEGER DEFAULT 0,
    created_at      INTEGER NOT NULL
);
```

On first startup with an empty tokens table, generate a random admin token,
print it to stdout with a clear marker, and insert it:

```
==============================================
ADMIN TOKEN (save this): cafe_adm_Xk9mPqR2...
==============================================
```

---

## CORS

Allow all origins in development. In production, restrict to the frontend origin.
Use `tower_http::cors::CorsLayer`.

---

## Environment variables

| Variable           | Default      | Description                        |
|--------------------|--------------|-------------------------------------|
| `CAFE_BUS_SOCKET`  | `/tmp/cafe-bus.sock` | Bus socket               |
| `CAFE_DB_PATH`     | `./cafe.db`  | SQLite path (shared with cafe-store)|
| `PORT`             | `3000`       | HTTP listen port                   |
| `CAFE_ADMIN_TOKEN` | *(generated)*| Seed admin token                   |
