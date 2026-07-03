# ObservableCAFE (Nominal Systems)

A Unix-philosophy reimplementation of the [ObservableCAFE](https://github.com/jorisvddonk/ObservableCAFE)
architecture as a suite of small, composable programs.

## Projects

| Crate / Module         | Language   | Role                                              |
|------------------------|------------|---------------------------------------------------|
| `cafe-types`           | Rust (lib) | Shared data model: Chunk, Annotation, ContentType |
| `cafe-bus`             | Rust       | Central reactive stream bus (Unix socket)         |
| `cafe-store`           | Rust       | SQLite session and history persistence            |
| `cafe-llm`             | Rust       | LLM backend bridge (Ollama, OpenAI-compat, etc.)  |
| `cafe-server`          | Rust       | HTTP API + SSE gateway                            |
| `cafe-tui`             | Rust       | Terminal UI client                                |
| `cafe-agent-runtime`   | Rust       | Agent loader, hot-reload, cron scheduler          |
| `cafe-telegram`        | Go         | Telegram bot bridge                               |
| `cafe-tts`             | Rust       | Voicebox TTS synthesis via cafe-bus               |
| `cafe-comfy`           | Rust       | ComfyUI image generation via cafe-bus             |
| `cafe-web`             | TypeScript | React frontend SPA                                |

## Prerequisites

- Rust (stable) — https://rustup.rs
- Go 1.22+ — https://go.dev/dl/
- Node.js 20+ — https://nodejs.org
- `just` — https://github.com/casey/just
- `process-compose` — https://github.com/F1bonacc1/process-compose

## Getting started

```sh
# Build everything
just build

# Start all services (dev mode)
just dev

# Run tests
just test
```

## Architecture

All services communicate via `cafe-bus` over a Unix socket at `/tmp/cafe-bus.sock`.
Wire format is newline-delimited JSON using types defined in `cafe-types`.

See [`docs/architecture.md`](docs/architecture.md) for the full design.

See [`docs/feature-matrix.md`](docs/feature-matrix.md) for where each feature lives (SDK, bus, etc.) and test coverage.

## Bus protocol

```
→  { "op": "subscribe", "session_id": "abc123" }
→  { "op": "publish",   "session_id": "abc123", "chunk": { ... } }
←  { "chunk": { "id": "...", "content_type": "text", "content": "hello", "producer": "...", "annotations": {} } }
```

## License

MIT
