# Architecture

## System overview

ObservableCAFE is a suite of small, composable Unix processes that collectively implement
a reactive multi-agent LLM platform. Processes communicate via a central message bus
(`cafe-bus`) over a Unix domain socket. No process calls another directly.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           cafe-bus                                       │
│              (Unix socket: /tmp/cafe-bus.sock)                           │
│                                                                          │
│  Sessions: { session_id → [ history: Vec<Chunk>, subscribers ] }        │
└────────┬──────────────┬──────────────┬───────────────┬──────────────────┘
         │              │              │               │
    subscribe_all   subscribe     subscribe        subscribe
         │          + publish     + publish        + publish
         ▼              │              │               │
    cafe-store          cafe-llm    cafe-agent-runtime  cafe-server
    (SQLite)        (LLM calls)  (agent lifecycle)  (HTTP gateway)
     cafe-tts         cafe-comfy
    (TTS/Voicebox)  (ComfyUI/img)
                                                        │
                                              ┌─────────┴──────────┐
                                         HTTP/SSE              HTTP/SSE
                                              │                    │
                                          cafe-web           cafe-tui
                                          (browser)          (terminal)
                                              │
                                         cafe-telegram
                                         (Telegram bot)
```

---

## Data flow for a user message

```
1.  User types a message in cafe-web
2.  cafe-web POSTs to cafe-server: POST /api/sessions/abc/chat { content: "Hello" }
3.  cafe-server creates a Chunk { content_type: text, content: "Hello", annotations: { chat.role: user } }
4.  cafe-server publishes the chunk to cafe-bus for session "abc"
5.  cafe-bus appends it to session history, broadcasts to all subscribers
6.  cafe-llm (subscribed to session "abc") receives the chunk
7.  cafe-llm builds conversation context from session history
8.  cafe-llm calls Ollama, streams back response tokens
9.  cafe-llm publishes each token as a Chunk { chat.role: assistant, chat.is_streaming: true }
10. cafe-bus broadcasts each token chunk
11. cafe-server (subscribed for this request) receives token chunks, forwards as SSE events
12. cafe-web receives SSE events, appends tokens to the message display
13. cafe-llm publishes a null chunk { chat.stream_complete: true }
14. cafe-server closes the SSE response
15. cafe-store (subscribed to everything) has persisted all chunks to SQLite throughout
```

---

## Startup order

Services must start in this order due to socket dependencies:

```
1. cafe-bus          — creates the socket; everything else waits for it
2. cafe-store        — connects to bus immediately
   cafe-llm          — connects to bus immediately (parallel with store)
   cafe-agent-runtime — connects to bus, initialises agents
3. cafe-server       — connects to bus; starts accepting HTTP after bus is ready
4. cafe-tts          — connects to bus (parallel with server, optional)
   cafe-comfy        — connects to bus (parallel with server, optional)
5. cafe-web          — static files, served by cafe-server or separately
   cafe-tui          — connects to cafe-server HTTP
   cafe-telegram     — connects to cafe-server HTTP
```

`process-compose` manages this via `depends_on` + readiness probes (see `process-compose.yml`).

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
