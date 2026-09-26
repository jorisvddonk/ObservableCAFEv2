# HTTP API Specification

`cafe-server` exposes this REST + SSE API. All clients (cafe-web, cafe-tui, cafe-telegram)
use this API exclusively — they do not connect to cafe-bus directly.

Base URL: `http://localhost:$PORT` (default port 4000)

---

## Authentication

All endpoints (except `GET /health`) require a bearer token:

```
Authorization: Bearer <token>
```

Tokens are managed via the admin API. The initial admin token is printed to stdout on
first startup and stored in the database.

---

## Sessions

### List sessions
`GET /api/sessions`

Response:
```json
[
  {
    "id": "abc123",
    "agent_id": "default",
    "display_name": "My Chat",
    "is_background": false,
    "ui_mode": "chat",
    "message_count": 14,
    "created_at": 1717123456789
  }
]
```

### Create session
`POST /api/sessions`

```json
{
  "agent_id": "default",
  "config": {
    "backend": "ollama",
    "model": "gemma3:1b",
    "system_prompt": "You are a helpful assistant."
  },
  "ui_mode": "chat"
}
```

Response `201`:
```json
{ "id": "abc123", "agent_id": "default" }
```

### Delete session
`DELETE /api/sessions/:id`

Response `204`.

### Fork session
`POST /api/sessions/:id/fork`

Creates a new session whose history (and retained transient chunks) are copied verbatim
from the session at `:id`, recording provenance. The original session is not modified.

Response `201`:
```json
{ "id": "fork-abc", "parent_id": "abc123" }
```

Response `404` if the parent session does not exist.

### Get session history
`GET /api/sessions/:id/history`

Response:
```json
{
  "session_id": "abc123",
  "chunks": [ { "id": "<chunk-id>" } ]
}
```

### Update session tags
`PATCH /api/sessions/:id/tags`

Request:
```json
{ "tags": ["work", "pinned"] }
```

### Session WebSocket
`GET /api/sessions/:id/ws?token=<token>`

A WebSocket that streams session chunks as JSON and accepts client messages
(see ADR-113). Requires the token as a query parameter (browsers cannot set
headers on a WebSocket handshake).

---

## Discovery

### List agents
`GET /api/agents`

Merged TOML + JS agent list. Each entry has `id`, `description`, `background`
and `source` (`"toml"` or `"js"`); a JS agent shadows a TOML agent of the same
name. See [JS agent reference](reference/js-agents.md).

### List models
`GET /api/models`

Models the configured LLM backend can reach (proxied from `cafe-llm`).

---

## Messaging

### Send a message (streaming)
`POST /api/sessions/:id/chat`

Request:
```json
{ "content": "Hello, world!" }
```

Response: `text/event-stream` (SSE). Each event is a chunk JSON object:

```
data: {"id":"...","content_type":"text","content":"Hello","annotations":{"chat.role":"assistant"},...}

data: {"id":"...","content_type":"null","annotations":{"chat.stream_complete":true},...}

```

The stream ends when a null chunk with `chat.stream_complete: true` is received,
or when the connection closes.

### Stream session activity (persistent)
`GET /api/sessions/:id/stream`

Returns a persistent SSE stream of all chunks published to the session's output stream,
including historical chunks replayed from the start (then live chunks).

Same event format as above. Does not close — the client must disconnect.

### Send a raw chunk
`POST /api/sessions/:id/chunks`

For sending non-chat content (binary files, config changes, signals).

Multipart form or JSON body:

```json
{
  "content_type": "null",
  "content": null,
  "annotations": {
    "config.type": "runtime",
    "config.model": "llama3:8b"
  }
}
```

For binary: multipart with `file` field. The server sets `content_type: binary`
and `mime_type` from the upload's content-type header.

Response `202`.

### Fetch web content (untrusted)
`POST /api/ext/sessions/:id/fetch`

```json
{ "url": "https://example.com/article" }
```

Fetches the URL via the cafe-web-fetch bus service (registered as a dynamic route).
Strips HTML tags, wraps as an untrusted text chunk, publishes to the session.
The user must explicitly trust it before the LLM can see it.

Response `200` with `{ "chunk_id": "..." }` on success, `502` on fetch failure.

### Trust / untrust a chunk
`PATCH /api/sessions/:id/chunks/:chunk_id`

```json
{ "trusted": true }
```

Response `200` with the updated chunk.

### Delete a chunk
`DELETE /api/sessions/:id/chunks/:chunk_id`

Response `204`.

### Get binary chunk content (proxied)
`GET /api/sessions/:id/chunks/:chunk_id/binary`

Proxies to `cafe-binary-store` using the chunk's `binary.read_url` annotation.

```json
{
  "url": "http://binary-store:4002/api/binary/<chunk_id>?token=<read_jwt>"
}
```

The client follows the redirect (or uses the URL directly for `<audio>`/`<img>` tags).

---

## Quickies (saved presets)

### List quickies
`GET /api/quickies`

### Create quickie
`POST /api/quickies`

```json
{
  "name": "Creative Writer",
  "description": "Help me write",
  "emoji": "✍️",
  "agent_id": "default",
  "starter_message": "Help me write a story about...",
  "config": { "system_prompt": "You are a creative writing assistant." },
  "ui_mode": "chat",
  "display_order": 0
}
```

### Delete quickie
`DELETE /api/quickies/:id`

---

## Admin

All admin endpoints require a token with `admin: true`.

### Token management
`GET  /api/admin/tokens`
`POST /api/admin/tokens` — `{ "description": "my token", "admin": false }`
`DELETE /api/admin/tokens/:id`

### Agent management
`GET /api/admin/agents` — background agent *sessions* currently registered

`POST /api/admin/agents/reload` — ask live agent runtimes to re-scan their
agent directories. Publishes a transient `cafe.flow.signal = "reload-agents"`
chunk on the `_cafe_agents` control session; `cafe-agent-js` reloads its
registry in place. Fire-and-forget (no per-agent confirmation), and runtimes
must be online — file watchers also reload on change. See
[ADR-132](adr-132-agent-reload-signal.md). Response:
`{"status":"reload requested","session":"_cafe_agents"}`

### System status
`GET /api/admin/status`

```json
{
  "uptime_seconds": 3600,
  "session_count": 5,
  "agent_count": 8,
  "bus_connected": true,
  "store_connected": true
}
```

---

## Health check
`GET /health`

No auth required.

```json
{ "status": "ok", "version": "0.1.0" }
```

---

## SSE event format

All SSE streams use the standard `data:` field only (no `event:` type field).
Each `data:` value is a complete JSON object on one line.

The client should handle these object shapes in the stream:
- Chunk object (has `content_type` field)
- `{ "type": "history_complete", "count": N }` — history replay finished
- `{ "type": "error", "message": "..." }` — non-fatal error
