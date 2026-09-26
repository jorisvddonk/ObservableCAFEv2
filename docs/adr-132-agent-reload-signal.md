# ADR-132: Agent reload control signal

**Status**: Implemented

**Date**: 2026-09-26

**Driver**: `POST /api/admin/agents/reload` was a stub — it returned
`{"status":"reload requested"}` and did nothing. `cafe-server` talks to the bus
only, so it had no way to ask the agent runtimes to re-scan their agent
directories. File watchers already reload on change, but that is not something
the HTTP surface can trigger.

**Context**: `cafe-agent-js` (and the frozen `cafe-agent-runtime`) are bus
clients; `cafe-server` cannot signal them directly. The bus already has a
`cafe.flow.signal` annotation convention (scheduler ticks). A control channel
was needed that any runtime could opt into without inventing new RPC methods or
coupling `cafe-server` to a specific runtime.

**Decision**: Use a well-known control session and a transient signal chunk.

- Two constants in `cafe-types::schema`: `AGENTS_SESSION = "_cafe_agents"` and
  `SIGNAL_RELOAD_AGENTS = "reload-agents"` (re-exported via `cafe-sdk::schema`).
- `cafe-server`'s `POST /api/admin/agents/reload` ensures the session exists and
  publishes a **transient** null chunk with
  `cafe.flow.signal = "reload-agents"`.
- `cafe-agent-js` ensures and subscribes to `_cafe_agents` at startup and, on
  the signal, re-scans its agent directories and replaces its registry
  wholesale (`loader::reload_all`). New files appear, changed files refresh,
  deleted files drop.

The signal is transient (ADR-005): it is not persisted and not replayed, so it
never triggers a spurious reload on reconnect, and a runtime that is offline
simply misses it (its file watcher still covers real changes).

**Consequences**:

- Positive: the admin endpoint does real work, with no new RPC surface.
- Positive: runtimes stay decoupled — any runtime can opt into the same signal.
- Positive: reload is also fixable by hand (`cafe-cli publish _cafe_agents
  --null --transient --annotation cafe.flow.signal=reload-agents`).
- Negative: fire-and-forget — the caller gets no per-agent confirmation.
- Negative: transient ⇒ best-effort; runtimes must be connected when the signal
  is sent.
- Negative: `cafe-agent-runtime` (frozen) does not yet act on the signal; TOML
  agents continue to rely on file watching/restart.

**Alternatives considered**:
- **Bus RPC to a runtime**: runtimes are not evaluators and expose no RPC;
  adding one would be new surface for a rare operation.
- **Touch agent files to trip the watchers**: hacky and racy.
- **`cafe-server` scanning directories itself**: it cannot affect running
  runtime state, so it would be a lie.

Related: [ADR-127](./adr-127-js-agent-runtime.md) (JS agent runtime),
[ADR-005](./adr-005-transient-sole-persistence-gate.md) (transient chunks),
[ADR-121](./adr-121-evaluator-schema-system.md) (the other well-known session).
