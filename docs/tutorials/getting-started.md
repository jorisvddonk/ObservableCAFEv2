# Tutorial 1: Getting started

Welcome! By the end of this tutorial you will have the whole ObservableCAFE stack
running on your machine and will have sent a message through it — from your
terminal, through the bus, through the LLM, and back.

You don't need to understand the architecture yet. Just follow the steps and
watch what happens at each one.

---

## What you'll need

- **Rust** (stable) — https://rustup.rs
- **Go 1.22+** — https://go.dev/dl/
- **Node.js 20+** — https://nodejs.org
- **`process-compose`** — https://github.com/F1bonacc1/process-compose
- A working LLM backend reachable at `http://localhost:6900` (OpenAI-compatible,
  e.g. the local proxy used by this repo's default config). The default model is
  `gemma3:1b`.

Check your tools:

```sh
rustc --version
go version
node --version
process-compose version
```

## Step 1 — Clone and build

```sh
git clone <your-fork-or-origin>
cd observablecafe

# Build all Rust crates
cargo build --workspace
```

This compiles every service: the bus, the store, the LLM bridge, the HTTP server,
the CLI, and the rest. It takes a while the first time.

## Step 2 — Start the stack

```sh
./start.sh
```

`start.sh` builds the Rust crates (if needed) and launches every service via
`process-compose` in the background:

- `cafe-bus` — the message bus on a Unix socket at `/tmp/cafe-bus.sock`
- `cafe-store` — SQLite persistence
- `cafe-llm` — LLM bridge
- `cafe-agent-runtime` — runs the agent pipelines
- `cafe-server` — HTTP API on port 4000
- `cafe-mcp-bridge` — MCP endpoint on port 3100
- and several more (binary-store, knowledgebase, tts, stt, comfy, …)

Check that the bus socket exists:

```sh
test -S /tmp/cafe-bus.sock && echo "bus is up"
```

### What if something didn't start?

Look at the process-compose UI, or stop everything and restart cleanly:

```sh
./stop.sh
./start.sh
```

## Step 3 — Look around with the CLI

`cafe-cli` is a command-line client for the bus. It's already built as part of the
workspace.

```sh
# What LLM models can the bus reach?
cafe-cli list-models

# What agents are registered?
cafe-cli list-agents
```

You should see the default agent (plus a few others), and a list of models from
your LLM backend.

## Step 4 — Send your first message

Create a session and send it a message:

```sh
SESSION=$(cafe-cli create-session --agent default)
echo "session: $SESSION"

cafe-cli chat "$SESSION" "Hello! What is the capital of France?"
```

`chat` streams the assistant's reply as JSON chunks. You'll see the LLM's tokens
arrive as `chat.is_streaming` chunks, then a final chunk with
`chat.stream_complete: true`.

## Step 5 — See the history

Everything you just sent was persisted. Fetch it back:

```sh
cafe-cli history "$SESSION"
```

You should see your user chunk, the assistant's text chunks, and the
stream-complete signal — in order.

## Step 6 — Talk to it over HTTP (the way the web app does)

The browser and the web SDK talk to `cafe-server`, not the bus directly.

```sh
# Server health (no auth needed)
curl http://localhost:4000/health

# List sessions
curl -H "Authorization: Bearer <token>" http://localhost:4000/api/sessions
```

> **The token**: on first startup, `cafe-server` prints an admin token to stdout
> and stores it. Find it in the process-compose logs, or wherever you captured
> startup output.

Stream a chat over SSE:

```sh
curl -N -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"content":"Hello over HTTP!"}' \
  http://localhost:4000/api/sessions/$SESSION/chat
```

## What just happened?

Trace it in [the architecture page](../architecture.md#data-flow-for-a-user-message):

1. You published a chunk to the session's input stream.
2. The bus broadcast it to every subscriber — including `cafe-store` (persisted it)
   and `cafe-agent-runtime` (kicked off the pipeline).
3. The pipeline's `llm` step called the LLM and streamed tokens back as chunks.
4. Your CLI saw the streamed chunks and printed them.

You've used every core concept already: **chunks**, **sessions**, **the bus**,
and an **agent pipeline**.

## Next steps

- [Tutorial 2: Your first agent](your-first-agent.md) — define your own agent.
- Need the exact commands for a task? See [Use the cafe-cli](../how-to/use-the-cli.md).
- Curious what a chunk *is*, exactly? See the [Data model spec](../spec-cafe.md).
