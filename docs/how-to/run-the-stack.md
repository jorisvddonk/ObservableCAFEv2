# How to: Run the stack

This guide shows you how to build, start, and stop the full ObservableCAFE stack,
and how to recover when something goes wrong.

---

## Prerequisites

- Rust, Go, Node.js, `process-compose` — see [Tutorial 1](../tutorials/getting-started.md#what-youll-need).

## Start the stack

### Quick start (debug)

```sh
./start.sh
```

This builds all Rust crates and starts every service via `process-compose` in the
background (detached). The bus socket appears at `/tmp/cafe-bus.sock`.

### Release mode (faster, for e2e tests)

```sh
cargo build --release
process-compose up -D
```

`process-compose.yml` at the repo root launches every service in dependency
order: `cafe-bus` first, then everything that depends on it, gated by readiness
probes.

## Verify it's healthy

```sh
# Bus socket exists
test -S /tmp/cafe-bus.sock && echo "bus up"

# HTTP server answers
curl http://localhost:4000/health

# CLI can reach the bus
cafe-cli list-agents
```

## What gets started

The stack (`process-compose.yml`) starts these services:

| Service | Purpose | Port/socket |
|---|---|---|
| `cafe-bus` | Central message bus | `/tmp/cafe-bus.sock` |
| `cafe-store` | SQLite persistence | — (bus client) |
| `cafe-binary-store` | Binary asset storage | 4002 |
| `cafe-llm` | LLM bridge | — (bus client) |
| `cafe-agent-runtime` | Agent pipelines (TOML, frozen) | — (bus client) |
| `cafe-agent-js` | JS agents (`agents-js/*.js`) | — (bus client) |
| `cafe-server` | HTTP gateway | 4000 |
| `cafe-mcp-bridge` | MCP endpoint | 3100 |
| `cafe-knowledgebase` | Vector search | — (bus client) |
| `cafe-web-fetch`, `cafe-rss`, `cafe-tts`, `cafe-stt`, `cafe-comfy`, `cafe-sheetbot`, `cafe-rot13`, `cafe-beacon` | Feature services | — (bus clients) |

A few processes are **disabled** by default (`cafe-telegram`, `cafe-demo`) because
they need credentials or are one-shot. Optional services like `cafe-comfy` run
but only do work when their backend is reachable.

## Watch the logs

`process-compose up` (foreground) shows live output. Since services log to
stdout/stderr, you can also inspect individual service logs through the
process-compose UI (default at `http://localhost:8080` when run with `--port`).

## Stop the stack

```sh
./stop.sh
```

This kills all services managed by process-compose on the standard port.

## Recovering from problems

### One service keeps crashing

Check its log line in process-compose. Common causes:

- **`cafe-llm` can't reach the model backend** — verify `OPENAI_URL` (default
  `http://localhost:6900`) is up and the model is installed. This repo's default
  config uses a local OpenAI-compatible proxy.
- **`cafe-knowledgebase` fails to embed** — verify `CAFE_KNOWLEDGEBASE_EMBED_URL`
  and that the embed model is available.
- **Missing optional backend (Voicebox, ComfyUI)** — disable the service in
  `process-compose.yml` (`disabled: true`) rather than fixing the backend.

### The bus socket is stale

If a previous run died uncleanly, `/tmp/cafe-bus.sock` may exist but be dead.
Stop everything, remove the stale socket, and restart:

```sh
./stop.sh
rm -f /tmp/cafe-bus.sock
./start.sh
```

### Start order matters

`process-compose` enforces ordering via `depends_on` + readiness probes, so
clients only start after the bus is ready. If you start services manually, always
start `cafe-bus` first.

## Reference

- [Architecture and startup order](../architecture.md#startup-order)
- [Environment variables](../AGENT.md#environment-variables-global)
- [How to: connect over iroh](connect-over-iroh.md) — run services against a bus on another machine
