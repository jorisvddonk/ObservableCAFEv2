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
```

---

## Build order (dependency graph)

Build and stabilise components in this order. Later components depend on earlier ones.

```
 1. cafe-types             — no dependencies; defines the data model everything else uses
 2. cafe-bus               — depends on cafe-types; the backbone all other services connect to
 3. cafe-store             — depends on cafe-types; connects to cafe-bus as a subscriber
 4. cafe-binary-store      — depends on cafe-types, cafe-sdk; binary asset hosting
 5. cafe-llm               — depends on cafe-types; LLM calls
 6. cafe-agent-runtime     — depends on cafe-types; connects to cafe-bus + cafe-store
 7. cafe-tts               — depends on cafe-types; connects to cafe-bus (optional — needs Voicebox)
 8. cafe-stt               — depends on cafe-types; connects to cafe-bus (optional — needs Voicebox)
 9. cafe-comfy             — depends on cafe-types; connects to cafe-bus (optional — needs ComfyUI)
10. cafe-sheetbot          — depends on cafe-types; connects to cafe-bus
11. cafe-web-fetch         — depends on cafe-types; connects to cafe-bus
12. cafe-knowledgebase     — depends on cafe-types; connects to cafe-bus
13. cafe-knowledgebase-index — depends on cafe-types; indexing tool
14. cafe-mcp-bridge        — depends on cafe-types; connects to cafe-bus
15. cafe-mcp-client        — depends on cafe-types; connects to cafe-bus
16. cafe-server            — depends on cafe-types, cafe-store; HTTP gateway (port 4000)
17. cafe-cli               — depends on cafe-sdk; CLI tool
18. cafe-web               — depends on HTTP API from cafe-server (TypeScript)
19. cafe-tui               — depends on HTTP API from cafe-server (Rust / ratatui)
20. cafe-telegram          — depends on HTTP API from cafe-server (Go)
21. cafe-dice              — depends on cafe-types; connects to cafe-bus
22. cafe-rot13             — depends on cafe-types; demo evaluator
```

**Start with `cafe-types`.** Every other component imports it. Its public API (structs,
enums, serialization) must be stable before writing logic in other crates.

---

## Language and framework choices

| Component              | Language   | Key crates / libraries                         |
|------------------------|------------|------------------------------------------------|
| cafe-types             | Rust       | `serde`, `serde_json`, `uuid`                  |
| cafe-bus               | Rust       | `tokio`, `tokio::net::UnixListener`, `serde_json` |
| cafe-store             | Rust       | `sqlx` (sqlite feature), `tokio`               |
| cafe-binary-store      | Rust       | `axum`, `tokio`, `jsonwebtoken`                |
| cafe-llm               | Rust       | `reqwest`, `tokio`, `futures-util`             |
| cafe-server            | Rust       | `axum`, `tokio`, `tower-http`                  |
| cafe-agent-runtime     | Rust       | `tokio`, `notify`, `tokio-cron-scheduler`      |
| cafe-tts               | Rust       | `reqwest`, `tokio`, `futures-util`             |
| cafe-stt               | Rust       | `reqwest`, `tokio`, `hound`                    |
| cafe-comfy             | Rust       | `reqwest`, `tokio`, `serde_json`               |
| cafe-sheetbot          | Rust       | `tokio`, `rhai`                                 |
| cafe-web-fetch         | Rust       | `reqwest`, `tokio`, `scraper`                  |
| cafe-knowledgebase     | Rust       | `lance`, `tokio`                                |
| cafe-mcp-bridge        | Rust       | `axum`, `tokio`                                |
| cafe-mcp-client        | Rust       | `tokio`, `serde_json`                          |
| cafe-cli               | Rust       | `clap`, `tokio`                                |
| cafe-tui               | Rust       | `ratatui`, `crossterm`, `reqwest`              |
| cafe-web               | TypeScript | React, Vite, `eventsource` (SSE)               |
| cafe-telegram          | Go         | `go-telegram-bot-api/telegram-bot-api`         |
| cafe-dice              | Rust       | `rand`                                         |
| cafe-rot13             | Rust       | —                                              |

Prefer `anyhow` for error handling in binaries. Use `thiserror` for library error types
in `cafe-types`. Always use `tracing` + `tracing-subscriber` for logging, never `println!`
in production paths.

---

## Environment variables (global)

| Variable                         | Default                        | Used by                         |
|----------------------------------|--------------------------------|---------------------------------|
| `CAFE_BUS_SOCKET`                | `/tmp/cafe-bus.sock`           | cafe-bus, all bus clients       |
| `CAFE_DB_PATH`                   | `./cafe.db`                    | cafe-store                      |
| `PORT`                           | `4000`                         | cafe-server                     |
| `CAFE_ADMIN_TOKEN`               | *(generated on first run)*     | cafe-server                     |
| `LLM_BACKEND`                    | `ollama`                       | cafe-llm                        |
| `OLLAMA_URL`                     | `http://localhost:11434`       | cafe-llm                        |
| `OLLAMA_MODEL`                   | `gemma3:1b`                    | cafe-llm                        |
| `OPENAI_URL`                     | `http://localhost:8000`        | cafe-llm                        |
| `OPENAI_API_KEY`                 | *(empty)*                      | cafe-llm                        |
| `MODEL_LIST_URLS`                | —                              | cafe-llm                        |
| `VOICEBOX_URL`                   | `http://127.0.0.1:17493`       | cafe-tts, cafe-stt              |
| `COMFY_URL`                      | `http://127.0.0.1:8188`        | cafe-comfy                      |
| `COMFY_WORKFLOW_PATH`            | `./cafe-comfy/workflow.json`   | cafe-comfy                      |
| `COMFY_WORKFLOW_INPUT_NODE`      | `6`                            | cafe-comfy                      |
| `TELEGRAM_TOKEN`                 | *(empty)*                      | cafe-telegram                   |
| `CAFE_SERVER_URL`                | `http://localhost:4000`        | cafe-telegram                   |
| `CAFE_KNOWLEDGEBASE_DB_PATH`     | `./knowledgebase.lance`        | cafe-knowledgebase              |
| `CAFE_KNOWLEDGEBASE_EMBED_URL`   | `http://localhost:8080/v1/embeddings` | cafe-knowledgebase      |
| `CAFE_KNOWLEDGEBASE_EMBED_MODEL` | `user.gemma3-embed`            | cafe-knowledgebase              |
| `CAFE_KNOWLEDGEBASE_EMBED_DIM`   | `1152`                         | cafe-knowledgebase              |
| `CAFE_MCP_SERVERS`               | `mcp-servers.toml`             | cafe-mcp-client                 |
| `CAFE_TRACE`                     | `0`                            | all (enables debug logging)     |

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
