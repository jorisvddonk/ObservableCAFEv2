# ADR-127: JS Agent Runtime

**Status**: Implemented (`cafe-agent-js`, `cafe-js-manifest`, `agents-js/`, `tests/js-agents-e2e.py`)

**Driver**: TOML `[[steps]]` pipelines (see [Agent config reference](./reference/agent-config.md))
can only express linear trigger chains (`user_message` → `llm_complete` →
`step_complete:<id>`) with `enabled_if` gating. Branching, loops, retries,
parallel evaluator calls, and JS-local state have no representation — every
such need becomes more step types and more string matching in
`cafe-agent-runtime`.

**Context**: The bus already correlates RPC by `call_id` (ADR-006): a transient
`{ns}.invoke` request chunk gets a transient response chunk with the same id.
`cafe-agent-runtime/src/executor.rs` (`dispatch_rpc`) and `tool_executor.rs`
(`execute`) both implement await-response-by-id. That mechanism maps 1:1 onto
JavaScript Promises, so pipelines can be plain `async/await` programs instead
of trigger tables.

**Decision**:

### 1. Pure-JS agents, no TOML

A JS agent is a plain script in `agents-js/*.js` defining two globals:

```js
const manifest = { name: "demo", mode: "stateless", /* … */ };
async function main(cafe) {
  for await (const event of cafe.events()) { /* … */ }
}
```

No modules (no loader/bundler ceremony), no TOML sidecar. Top-level code must
be side-effect free — the host evaluates the whole file to reach `main`.
The manifest schema and its parser live in `cafe-js-manifest` (shared by the
executor and `cafe-server`), so there is exactly one definition of the format.
See [JS agent reference](./reference/js-agents.md).

### 2. `rquickjs` with `futures` (QuickJS-NG under the hood)

`rquickjs` ≥ 0.12 already bundles QuickJS-NG — there is no separate
quickjs-ng crate decision. The `futures` feature provides `AsyncRuntime` /
`AsyncContext` with native Promise↔Rust-Future bridging: an `Async`-wrapped
Rust function returning a future becomes a JS Promise, and
`Promise::into_future` awaits JS settlement from Rust. All host↔agent values
cross as JSON strings; domain errors travel as `{"ok":false}` envelopes that
the prelude unwraps into real JS `Error`s, so Rust never constructs a QuickJS
exception.

### 3. Events as an async generator, RPC as Promises — no handler exports

There are deliberately no `onUserMessage` / `onLlmComplete` exports. The host
exposes `cafe.nextEvent()` (backed by a Rust channel) and a JS prelude
defines `cafe.events()` over it, so orchestration is one `for await` loop.
`cafe.invoke(evaluator, params)` and `cafe.rpc(method, params)` each mint a
UUID, publish the transient request, and resolve/reject on the matching
`call_id` response — concurrent awaits multiplex safely. Error/timeout
responses reject, so agents use `try/catch` and `Promise.all` directly.

### 4. Two modes, agent's choice

- `stateless`: fresh runtime per event, one-shot channel. State lives only in
  bus history. Safe default.
- `stateful`: one runtime per session fed by a live channel; JS locals
  survive across events. The supervisor respawns the runtime (same channel)
  when the file's hash changes or the previous `main()` returned, and tears
  it down on `SessionDeleted`.

### 5. New binary, frozen TOML runtime

`cafe-agent-js` supersedes `cafe-agent-runtime`, which is frozen (bugfixes
only). Sessions route by `agent_id`; a name defined in both worlds resolves
to the JS agent. The two binaries coexist during the migration.

JS shadowing is enforced at **runtime**, not only in listings:
`cafe-agent-runtime` scans the JS agent directories (`./agents-js` +
`CAFE_JS_AGENT_PATHS`) and excludes any TOML agent whose name a JS manifest
claims. Without this, both runtimes attach to the same session and every step
runs twice (observed: 4 evaluator chunks instead of 2). Migration therefore
means "add the JS agent"; the TOML file may stay until the tests that drive
it through the legacy runtime are migrated too.

### 6. Sandboxing posture (Phase 3 level)

No fs/net/timers in the JS context — the only I/O is bus-mediated (`invoke`,
`rpc`, `publishText`, `config`, `log`). Stateless runs have a 120 s overall
budget; every RPC await has a per-agent timeout; promise rejections are
tracked into error chunks (`error.source: js-agent`), never silent.

**Consequences**:

- Positive: branching/loop/retry/`Promise.all` replace `step_complete`
  chaining and `enabled_if`; adding orchestration needs no Rust changes.
- Positive: stateful agents hold conversational state without history scans.
- Positive: agent files are testable with the same temp-bus E2E harness
  (`tests/js-agents-e2e.py`, in CI).
- Negative: each JS run pays a QuickJS startup (~ms) in stateless mode;
  hot paths should prefer stateful.
- Negative: QuickJS futures are `!Send` — the supervisor runs on a `LocalSet`;
  `run_with_reconnect` (which needs `Send`) cannot be reused there.
- Negative: `tokio-cron-scheduler 0.10` (cron 0.12) requires 7-field cron with
  seconds — 5-field expressions are rejected. (This also affects the legacy
  `rss-summarizer.toml` schedule, which never registers.)

**Alternatives considered**:
- **Rhai/Deno**: Rhai has no `async/await`; Deno (deno_core/V8) is far heavier
  to embed than QuickJS for untrusted-agent orchestration.
- **ES modules with imports**: rejected — loader/resolver ceremony for no
  current need; agents share code by convention, not imports.
- **Extending TOML steps** (conditionals, loops): rejected — reinvents a worse
  programming language inside a config format.
- **Raw `cafe.rpc` only (no `cafe.tool`)**: rejected — the follow-up LLM turn
  needs the bus-visible `cafe.tool.result` chunk and readable text, so the
  host publishes both inside `cafe.tool` (mirroring `tool_executor`).

Related: [ADR-006](./adr-006-jsonrpc-over-bus-annotations.md) (call_id correlation),
[ADR-121](./adr-121-evaluator-schema-system.md) (evaluator schemas),
[ADR-123](./adr-123-session-forking.md) (history gating),
[JS agent reference](./reference/js-agents.md).
