# ObservableCAFE — AI Agent Context

This document is the entry point for an AI coding agent working in this repository.
Read this first, then read the specific doc for the component you are building.

---

## What this project is

ObservableCAFE is a multi-agent LLM execution platform built on the **CAFE architecture**
(Chunks, Annotations, Functions/Evaluators). It is a Rust/Go/TypeScript monorepo that
reimplements the original TypeScript [ObservableCAFE](https://github.com/jorisvddonk/ObservableCAFE)
as a suite of small Unix-philosophy programs that communicate over a shared message bus.

The core idea: LLM agents should generate scripts/plans upfront rather than reasoning
step-by-step in a loop. Data flows through the system as immutable **Chunks** (text,
binary, or null) annotated with metadata. Pipelines are declarative — they describe
*what* should happen to data, not *how*.

---

## Repository layout

```
observablecafe/
├── Cargo.toml              # Rust workspace root (all Rust crates listed here)
├── Justfile                # Task runner: just build / just dev / just test
├── process-compose.yml     # Local process orchestration (like docker-compose for binaries)
├── docs/
│   ├── AGENT.md            # ← you are here
│   ├── architecture.md     # System diagram and startup order
│   ├── spec-cafe.md        # CAFE data model spec (Chunk, Annotation, Evaluator)
│   ├── spec-bus-protocol.md # Wire protocol for cafe-bus IPC
│   ├── spec-http-api.md    # HTTP API exposed by cafe-server
│   ├── cafe-types.md       # Build guide for the shared types crate
│   ├── cafe-bus.md         # Build guide for the bus
│   ├── cafe-store.md       # Build guide for the persistence service
│   ├── cafe-llm.md         # Build guide for the LLM bridge
│   ├── cafe-server.md      # Build guide for the HTTP server
│   ├── cafe-agent-runtime.md # Build guide for the agent host
│   ├── cafe-tui.md         # Build guide for the terminal client
│   ├── cafe-telegram.md    # Build guide for the Telegram bridge (Go)
│   ├── cafe-comfy.md       # Build guide for the ComfyUI image generation bridge
│   └── cafe-web.md         # Build guide for the React frontend (TypeScript)
├── cafe-types/             # Rust library: shared data model
├── cafe-bus/               # Rust binary: central message bus
├── cafe-store/             # Rust binary: SQLite persistence
├── cafe-llm/               # Rust binary: LLM backend bridge
├── cafe-server/            # Rust binary: HTTP API + SSE gateway
├── cafe-tui/               # Rust binary: terminal UI client
├── cafe-agent-runtime/     # Rust binary: agent loader + scheduler
├── cafe-telegram/          # Go binary: Telegram bot bridge
├── cafe-tts/               # Rust binary: Voicebox TTS bridge
├── cafe-comfy/             # Rust binary: ComfyUI image generation bridge
└── cafe-web/               # TypeScript/React: browser frontend
```

---

## Build order (dependency graph)

Build and stabilise components in this order. Later components depend on earlier ones.

```
1. cafe-types        — no dependencies; defines the data model everything else uses
2. cafe-bus          — depends on cafe-types; the backbone all other services connect to
3. cafe-store        — depends on cafe-types; connects to cafe-bus as a subscriber
4. cafe-llm          — depends on cafe-types; connects to cafe-bus
5. cafe-agent-runtime — depends on cafe-types; connects to cafe-bus
6. cafe-tts           — depends on cafe-types; connects to cafe-bus (optional — needs Voicebox)
7. cafe-comfy         — depends on cafe-types; connects to cafe-bus (optional — needs ComfyUI)
8. cafe-server        — depends on cafe-types; connects to cafe-bus + exposes HTTP
7. cafe-tui          — depends on HTTP API from cafe-server
8. cafe-telegram     — depends on HTTP API from cafe-server (Go, independent)
10. cafe-web          — depends on HTTP API from cafe-server (TypeScript, independent)
11. cafe-telegram     — depends on HTTP API from cafe-server (Go, independent)
```

**Start with `cafe-types`.** Every other component imports it. Its public API (structs,
enums, serialization) must be stable before writing logic in other crates.

---

## Language and framework choices

| Component          | Language   | Key crates / libraries                         |
|--------------------|------------|------------------------------------------------|
| cafe-types         | Rust       | `serde`, `serde_json`, `uuid`                  |
| cafe-bus           | Rust       | `tokio`, `tokio::net::UnixListener`, `serde_json` |
| cafe-store         | Rust       | `sqlx` (sqlite feature), `tokio`               |
| cafe-llm           | Rust       | `reqwest`, `tokio`, `futures-util`             |
| cafe-server        | Rust       | `axum`, `tokio`, `tower-http`                  |
| cafe-agent-runtime | Rust       | `tokio`, `notify`, `tokio-cron-scheduler`      |
| cafe-tts           | Rust       | `reqwest`, `tokio`, `futures-util`             |
| cafe-comfy         | Rust       | `reqwest`, `tokio`, `serde_json`               |
| cafe-tui           | Rust       | `ratatui`, `crossterm`, `reqwest`              |
| cafe-telegram      | Go         | `go-telegram-bot-api/telegram-bot-api`         |
| cafe-web           | TypeScript | React, Vite, `eventsource` (SSE)               |

Prefer `anyhow` for error handling in binaries. Use `thiserror` for library error types
in `cafe-types`. Always use `tracing` + `tracing-subscriber` for logging, never `println!`
in production paths.

---

## Environment variables (global)

| Variable              | Default                  | Used by                    |
|-----------------------|--------------------------|----------------------------|
| `CAFE_BUS_SOCKET`     | `/tmp/cafe-bus.sock`     | cafe-bus, all bus clients  |
| `CAFE_DB_PATH`        | `./cafe.db`              | cafe-store                 |
| `LLM_BACKEND`         | `ollama`                 | cafe-llm                   |
| `OLLAMA_URL`          | `http://localhost:11434` | cafe-llm                   |
| `OLLAMA_MODEL`        | `gemma3:1b`              | cafe-llm                   |
| `OPENAI_URL`          | `http://localhost:8000`  | cafe-llm                   |
| `OPENAI_API_KEY`      | *(empty)*                | cafe-llm                   |
| `PORT`                | `3000`                   | cafe-server                |
| `CAFE_ADMIN_TOKEN`    | *(generated on first run)* | cafe-server              |
| `TELEGRAM_TOKEN`      | *(empty)*                | cafe-telegram              |
| `VOICEBOX_URL`        | `http://127.0.0.1:17493` | cafe-tts                  |
| `COMFY_URL`           | `http://127.0.0.1:8188`  | cafe-comfy                |
| `COMFY_WORKFLOW_PATH` | `./cafe-comfy/workflow.json` | cafe-comfy                |
| `COMFY_WORKFLOW_INPUT_NODE` | `6`               | cafe-comfy                |
| `CAFE_TRACE`          | `0`                      | all (enables debug logging)|

---

## Key design rules to preserve

1. **Chunks are immutable.** Never mutate a chunk. Produce a new chunk with updated
   annotations instead.
2. **The bus is the only shared state.** Services do not call each other directly.
   All coordination goes through chunk streams on the bus.
3. **History is the source of truth.** Session state is derived by scanning the
   chunk history, not stored separately.
4. **Errors are out-of-band.** Errors go to an error stream, never mixed into the
   data stream.
5. **All services must handle `SIGTERM` gracefully.** Flush in-flight work, close
   the bus connection cleanly, exit with code 0.
6. **Log to stdout/stderr only.** No log files. systemd/process-compose captures output.

---

## Where to start if building a specific component

Read `docs/AGENT.md` (this file), then read the corresponding `docs/<component>.md`.
Each component doc contains: purpose, inputs/outputs, data flow, public interface,
suggested file structure, and implementation notes.
