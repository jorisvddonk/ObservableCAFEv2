# Architecture

## System overview

ObservableCAFE is a suite of small, composable Unix processes that collectively implement
a reactive multi-agent LLM platform. Processes communicate via a central message bus
(`cafe-bus`) over a Unix domain socket. No process calls another directly.

```mermaid
graph TB
    bus["cafe-bus (Unix socket)"]

    subgraph "Bus-connected services"
        store[cafe-store<br/>SQLite persistence]
        llm[cafe-llm<br/>LLM calls]
        agent[cafe-agent-runtime<br/>agent lifecycle]
        tts[cafe-tts<br/>TTS / Voicebox]
        comfy[cafe-comfy<br/>ComfyUI / img]
        server[cafe-server<br/>HTTP gateway]
    end

    bus --> store
    bus --> llm
    bus --> agent
    bus --> tts
    bus --> comfy
    bus --> server

    subgraph "Clients"
        web[cafe-web<br/>browser]
        tui[cafe-tui<br/>terminal]
        tg[cafe-telegram<br/>Telegram bot]
    end

    server -- HTTP/SSE --> web
    server -- HTTP/SSE --> tui
    server -- HTTP/SSE --> tg
```

---

## Data flow for a user message

```mermaid
sequenceDiagram
    participant User as User (browser)
    participant Web as cafe-web
    participant Svr as cafe-server
    participant Bus as cafe-bus
    participant LLM as cafe-llm
    participant Store as cafe-store

    User->>Web: type message
    Web->>Svr: POST /api/sessions/abc/chat {content:"Hello"}
    Svr->>Bus: publish chunk (chat.role: user)
    Bus->>LLM: broadcast chunk
    Bus->>Store: broadcast chunk (persisted)
    Bus->>Svr: broadcast chunk (SSE forward)
    Svr->>Web: SSE: text chunk
    LLM->>Bus: publish tokens (chat.is_streaming)
    Bus->>Svr: broadcast tokens
    Svr->>Web: SSE: streaming tokens
    LLM->>Bus: publish stream_complete
    Bus->>Svr: broadcast complete
    Svr->>Web: SSE: stream complete
```

---

## Startup order

```mermaid
graph LR
    bus[cafe-bus] --> store[cafe-store]
    bus --> llm[cafe-llm]
    bus --> agent[cafe-agent-runtime]
    bus --> tts[cafe-tts]
    bus --> comfy[cafe-comfy]
    bus --> server[cafe-server]
    server --> web[cafe-web]
    server --> tui[cafe-tui]
    server --> tg[cafe-telegram]

    style bus fill:#4a6,color:#fff
    style store fill:#46a,color:#fff
    style llm fill:#46a,color:#fff
    style agent fill:#46a,color:#fff
    style server fill:#46a,color:#fff
```

Dependencies are enforced by `process-compose` via `depends_on` + readiness probes (see `process-compose.yml`).

---

## Key concepts

See `docs/spec-cafe.md` for the full data model specification.

**Chunk** — Immutable unit of data. Has an ID, content type (text/binary/null),
content, producer, annotations, and timestamp.

**Annotation** — Key-value metadata on a chunk. Keys use dot-namespaced strings
(`chat.role`, `config.type`, `security.trust-level`). Full list in spec-cafe.md.

**Session** — Ordered history of chunks with input/output/error streams.
State is derived from history, not stored separately.

**Evaluator** — Function that takes a chunk + history and produces zero or more chunks.

**Agent** — Pipeline builder. Wires evaluators into a data flow for a session type.
In this implementation, agents are TOML files that declare a pipeline of named evaluators.

---

## IPC protocol

Full specification in `docs/spec-bus-protocol.md`.

Wire format: newline-delimited JSON (NDJSON) over Unix domain socket.

Client → bus:
```json
{ "op": "publish", "session_id": "abc", "chunk": { ...chunk fields... } }
{ "op": "subscribe", "session_id": "abc" }
{ "op": "create_session", "session_id": "abc", "agent_id": "default", "config": {} }
```

Bus → client:
```json
{ "event": "chunk", "session_id": "abc", "chunk": { ...chunk fields... } }
{ "event": "history_complete", "session_id": "abc", "count": 42 }
```

---

## HTTP API

Full specification in `docs/spec-http-api.md`.

Base URL: `http://localhost:3000`  
Auth: `Authorization: Bearer <token>`

Key endpoints:
- `GET  /api/sessions` — list sessions
- `POST /api/sessions` — create session
- `POST /api/sessions/:id/chat` — send message, stream response (SSE)
- `GET  /api/sessions/:id/stream` — persistent SSE stream of all activity
- `GET  /health` — health check (no auth)

---

## Repository layout

See the [README](../README.md#projects) for the current list of projects with descriptions and languages.

---

## Design principles

1. **Chunks are immutable.** Produce new chunks with updated annotations; never mutate.
2. **The bus is the only shared state.** Services do not call each other directly.
3. **History is the source of truth.** Derive state by scanning chunk history.
4. **Errors are out-of-band.** Errors go to an error stream, never the data stream.
5. **All services must handle SIGTERM gracefully.** Flush work, close connections, exit 0.
6. **Log to stdout/stderr only.** Use `tracing` in Rust, `log/slog` in Go.
