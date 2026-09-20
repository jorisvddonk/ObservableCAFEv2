# JS agent reference

A **JS agent** is a plain script in `agents-js/*.js` executed by `cafe-agent-js`.
It replaces the TOML `[[steps]]` pipeline with a single `async function
main(cafe)` — orchestration is ordinary `async/await` JavaScript. This page is
the complete contract. For walkthroughs, see [Tutorial: your first JS
agent](../tutorials/your-first-js-agent.md) and [Tutorial: a JS tool-calling
agent](../tutorials/js-tool-calling-agent.md).

---

## File layout

```js
// agents-js/demo.js
const manifest = {
  name: "demo",
  description: "JS echo via rot13 RPC — hello world without an LLM",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      const res = await cafe.invoke("rot13", { text: event.text });
      await cafe.publishText(res.text);
    }
  }
  return "demo: done";
}
```

Rules:

- Plain script, **no modules** — no `import`/`export`. The host evaluates the
  whole file and calls the global `main`.
- Top-level code must be **side-effect free** (define `manifest` and `main`
  only). Manifest extraction evaluates the file in a throwaway runtime.
- Exactly one `manifest` object and one `main(cafe)` function.

## `manifest` fields

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | *(required)* | Unique identifier. Session ID for background agents. A JS agent shadows a TOML agent of the same name — both in `GET /api/agents` and at runtime: `cafe-agent-runtime` excludes shadowed TOML agents so a name executes only in `cafe-agent-js` (otherwise both runtimes attach and every step runs twice). |
| `description` | string | `""` | Human-readable purpose. |
| `background` | bool | `false` | If `true`, the host auto-creates the session at boot and seeds config. |
| `allows_reload` | bool | `true` | If `false`, file changes are logged and ignored. |
| `persists_state` | bool | `true` | Informational (the store persists what the bus carries); kept for TOML parity. |
| `mode` | string | `"stateless"` | `"stateless"` (fresh runtime per event) or `"stateful"` (one runtime per session; JS locals survive). |
| `schedule` | string | `None` | Cron for background agents (tick events). **7-field with seconds** (e.g. `"*/10 * * * * * *"`); 5-field is rejected. |
| `rpc_timeout_secs` | number | `30` | Per-RPC await timeout. Rejections are catchable (`try/catch`). |
| `initial_config` | object | `{}` | Annotations for the config-seeding null chunk, published when the host attaches to a session (background creation or user-session attach; re-attach duplicates are harmless under later-wins merging). `config.type = "runtime"` is injected when absent. |

Parsed and validated by `cafe-js-manifest` (shared by the executor and
`cafe-server`); unknown `mode` values and empty names are load errors.

## Events

`cafe.events()` is an async generator of:

```ts
{ type: "user_message" | "llm_complete" | "tick", text: string }
```

| Bus condition | Event |
|---|---|
| Non-transient user text/`binary_ref` chunk | `user_message` (`text` = content) |
| Assistant chunk with `chat.stream_complete` | `llm_complete` |
| Null chunk with `cafe.flow.signal = "tick"` (cron or manual) | `tick` |

Transient RPC plumbing never surfaces as events. The generator ends when the
session's stream closes (stateless: after the single event).

## `cafe` API

All boundary values are JSON. Every async method returns a real Promise.

| Member | Signature | Meaning |
|---|---|---|
| `cafe.events()` | `AsyncGenerator<Event>` | The event stream (above). |
| `cafe.invoke(evaluator, params)` | `Promise<any>` | `{evaluator}.invoke` RPC. Resolves with `result`, rejects with `Error` (message + optional `code`) on RPC error/timeout. |
| `cafe.rpc(method, params)` | `Promise<any>` | Raw RPC for any bus method (e.g. `dice.roll`, custom service methods). Same resolve/reject contract. No side publishes. |
| `cafe.tool(name, params)` | `Promise<any>` | Bus-RPC tool call: dispatches `{name}`, then publishes the bus-visible `cafe.tool.result` chunk plus the readable `Tool call completed…` assistant text a follow-up LLM turn reads. Resolves with the tool output. MCP-provider tools are out of scope (they stay with `cafe-mcp-client`). |
| `cafe.publishText(text)` | `Promise<{published:true}>` | Assistant text chunk from the JS host. |
| `cafe.config()` | `Promise<object>` | Merged runtime config: all `config.type == "runtime"` null chunks, later wins per key. Snapshot per event (stateless) / refreshed per event (stateful). |
| `cafe.log(msg)` | `void` | Host log (`tracing::info`). |
| `cafe.sessionId` / `cafe.agentId` | `string` | Current session / agent. |

Params pass straight through: session-scoped evaluators (`llm`) read
history and config from the session the request lands in, so `await
cafe.invoke("llm", {})` suffices; method-specific params (`text` for
`rot13`/`tts`, `count`/`sides` for `dice.roll`) are the agent's job to
compose, typically from the event and `cafe.config()`.

Tool-calling agents parse `<|tool_call|>` markers and use `cafe.tool` — see
the [tool-calling tutorial](../tutorials/js-tool-calling-agent.md). Tool
definitions for the LLM travel in `initial_config` under `tools.available`
(same shape as TOML).

### Promise semantics

Each `invoke`/`rpc` mints a UUID `call_id`, publishes the transient request,
and resolves on the response chunk with the matching id (ADR-006).
Concurrent awaits are safe (`Promise.all` fans out over independent calls).
Timeout (`manifest.rpc_timeout_secs`) and bus disconnects reject.

### Modes

- **stateless** (default): one runtime per event. No JS state survives; all
  durable state is bus history. Overall 120 s budget per run.
- **stateful**: one runtime per session. `main` should loop forever; if it
  returns, the supervisor restarts it on the next event with fresh state.
  Hot-reload respawns the runtime on the same stream (JS state resets).
  No overall budget; RPC awaits stay bounded.

### Errors

JS throws become Rust errors attributed to the agent and published as an
error chunk (`cafe.error.message`, `error.source: js-agent`) — never silent.
A stateless failure affects one event; a stateful failure restarts that
session's runtime on the next event.

### Hot-reload

`cafe-agent-js` watches `agents-js/*.js`. Valid reloads apply to subsequently
read sources (stateless: next event; stateful: respawn on next event, state
resets). `allows_reload: false` and invalid files are logged and ignored.
Schedule changes need a restart.

### Environment

| Variable | Default | Meaning |
|---|---|---|
| `CAFE_BUS_SOCKET` | `/tmp/cafe-bus.sock` | Bus socket. |
| `CAFE_JS_AGENT_PATHS` | — | Extra agent dirs (colon-separated) beyond `./agents-js`. |
| `CAFE_JS_RPC_TIMEOUT_SECS` | `30` | Default RPC timeout (per-agent `rpc_timeout_secs` wins). |

## Example agents

| Agent | Highlights |
|---|---|
| `agents-js/demo.js` | Stateless rot13 echo; the minimal loop. |
| `agents-js/default.js` | Standard chat agent (migrated from `agents/default.toml`). |
| `agents-js/rot13.js` | ROT13 echo (migrated from `agents/rot13.toml`). |
| `agents-js/counter.js` | Stateful counter in JS-local state. |
| `agents-js/stt.js` | Audio transcription via `stt.invoke` (agent-driven, [ADR-128](../adr-128-stt-agent-driven.md)). |
| `agents-js/fetch.js` | `!fetch <url>` via `web-fetch.invoke`; content is untrusted. |
| `agents-js/knowledgebase.js` | RAG: `knowledgebase.search` (raw RPC) → context chunk → `llm`. |
| `agents-js/dice-llm.js` | LLM tool-calling round-trip via `cafe.tool`. |
| `agents-js/heartbeat.js` | Background + `initial_config` + `cafe.config()` + manual ticks. |
| `agents-js/ticker.js` | Cron schedule (`*/10 * * * * * *`). |

Type declarations for editors: [`agents-js/cafe.d.ts`](../../agents-js/cafe.d.ts).

---

## Reference

- [ADR-127: JS Agent Runtime](../adr-127-js-agent-runtime.md)
- [Data model spec, §5 Agent](../spec-cafe.md#5-agent)
- [Tutorial: your first JS agent](../tutorials/your-first-js-agent.md)
- [Tutorial: a JS tool-calling agent](../tutorials/js-tool-calling-agent.md)
- [How to: debug a JS agent](../how-to/debug-js-agent.md)
- [ADR-006: JSON-RPC over bus annotations](../adr-006-jsonrpc-over-bus-annotations.md)
- [Agent config reference (TOML, legacy)](agent-config.md)
