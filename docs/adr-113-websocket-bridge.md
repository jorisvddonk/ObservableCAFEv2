# ADR-113: WebSocket Bridge for HTTP Clients

**Status**: Implemented

## Context

HTTP clients connecting through cafe-server could publish chunks via
`POST /api/sessions/:id/chunks`, but they could not participate in the
BinaryRef publishing flow. BinaryRef requires a persistent bus connection
with a unique connection ID so that cafe-binary-store can reply with
write credentials via `direct_to` messages.

Without this, HTTP clients had no way to:
- Receive `source.connection` tagging (required for binary-store replies)
- Get `direct_to` messages carrying write JWTs
- Upload binary data to the binary store as a non-bus client

## Decision

Add a WebSocket endpoint at `GET /api/sessions/:id/ws` that gives HTTP
clients a persistent, bidirectional connection to the session bus.

### Endpoint

```
GET /api/sessions/:id/ws?token=<auth>
```

Authentication via `?token=` query param (same as SSE stream, since
WebSocket handshake cannot set custom headers).

The endpoint uses the standard cafe-server `AuthUser` extractor for
authentication, supporting both `Authorization: Bearer` header for the
initial HTTP upgrade request and `?token=` for clients that can only
set query params.

### Protocol

**Server → Client** (JSON messages):

```json
{"event":"chunk","chunk":{"id":"...","content_type":"text","data":"hello"}}
{"event":"history_complete","count":0}
{"event":"error","message":"...","code":"..."}
```

**Client → Server** (JSON messages):

```json
{"op":"publish","chunk":{"content_type":"binary_ref","mime_type":"audio/wav","annotations":{...}}}
{"op":"subscribe","session_id":"<new>"}
```

### BinaryRef flow

```
HTTP Client                     cafe-server                      cafe-bus           cafe-binary-store
    │                               │                               │                      │
    │  WS /api/sessions/:id/ws      │                               │                      │
    │ ──────────────────────────►   │                               │                      │
    │                               │  subscribe(session)           │                      │
    │                               │ ──────────────────────────►   │                      │
    │                               │  ◄── Chunks + HistoryComplete │                      │
    │  ◄── chunks ─────────────────  │                               │                      │
    │                               │                               │                      │
    │  {"op":"publish",             │                               │                      │
    │   chunk:{content_type:        │  publish(chunk)                │                      │
    │    "binary_ref",...}}         │  (auto-tagged with             │                      │
    │ ──────────────────────────►   │   source.connection=c-N)       │                      │
    │                               │ ──────────────────────────►   │                      │
    │                               │                               │ subscribe_filtered   │
    │                               │                               │ ──────────────────►  │
    │                               │                               │ ◄── BinaryRef chunk  │
    │                               │                               │                      │
    │                               │                               │ publish_direct(      │
    │                               │                               │   target=c-N,        │
    │                               │                               │   write_creds)        │
    │                               │                               │ ─────────────────►   │
    │                               │  ◄── direct_to(c-N) ────────  │                      │
    │  ◄── chunk(write_url,         │                               │                      │
    │        write_token) ──────────│                               │                      │
    │                               │                               │                      │
    │  PUT /api/binary/{id}         │                               │                      │
    │  (with write_token)           │                               │                      │
    │ ─────────────────────────────────────────────────────────► cafe-binary-store       │
```

The key insight: when cafe-server publishes a chunk via its bus connection,
the bus auto-injects `cafe.source.connection` with the server's connection
ID. cafe-binary-store sends write credentials back via `publish_direct`
addressed to that connection ID. The bus delivers them to cafe-server's
subscription, which forwards them to the WebSocket client.

### Server implementation

**File:** `cafe-server/src/handlers/ws_handler.rs`

Uses `tokio::select!` to multiplex between:
- Incoming bus messages → forward as JSON events to WebSocket
- Incoming WebSocket messages → execute actions (`publish`, `subscribe`)

The `subscribe` action switches to a different session by establishing a
new bus subscription, replacing the existing one.

### SDK client

**File:** `cafe-sdk/src/ws.rs` (feature: `ws-client`)

```rust
use cafe_sdk::ws::WsClient;

let (client, mut rx) = WsClient::connect(
    "http://localhost:4000",
    "session-id",
    "auth-token",
).await?;

// Read history + live chunks
while let Some(ServerMessage::Chunk { chunk, .. }) = rx.recv().await {
    println!("{}", chunk.content.unwrap_or_default());
}

// Publish a chunk
client.publish(&Chunk::new_text("hello", "test")).await?;
```

### Route

```
GET /api/sessions/:id/ws → ws_handler::ws_session
```

Added after the binary chunk route in `router.rs`.

### Dependencies

| Crate | Feature | Reason |
|---|---|---|
| cafe-server | axum with `ws` | WebSocket upgrade |
| cafe-sdk | `ws-client` (optional) | `tokio-tungstenite` for WebSocket client |

### Consequences

- HTTP clients can now publish BinaryRef chunks and receive write credentials
- No changes to cafe-binary-store or cafe-bus — the existing `source.connection`
  and `direct_to` mechanism works unchanged
- No changes to existing REST endpoints
- WebSocket protocol is simple JSON — any WS client can use it
- SDK provides a typed Rust client behind the `ws-client` feature
- Session switching allows a single WS connection to multiplex across sessions
