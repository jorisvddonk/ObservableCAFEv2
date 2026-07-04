# ADR-108: Annotation key namespace (`cafe.*`)

**Status**: Implemented (`c0f48e2`)

**Context**: Annotation keys are flat strings (`transient`, `direct_to`, `binary.write_url`, `flow.signal`, etc.). Every service and agent shares the same namespace. As the system grows, this creates collision risk — an agent TOML might define a `config.foo` annotation that happens to be interpreted by a future platform component. There's no way to distinguish "this annotation has bus-level meaning" from "this annotation is agent-authored data."

**Decision**: Annotation keys that the bus or platform services interpret as instructions are prefixed with `cafe.`. Data annotations — those that describe content or are authored by agents — remain unprefixed.

**`cafe.*` (platform instructions):**
- `cafe.transient`, `cafe.transient.retain_secs` — persistence gate
- `cafe.source.connection` — bus-injected publisher identity
- `cafe.direct_to` — connection-level routing
- `cafe.mutates.target_id` — mutation protocol
- `cafe.jsonrpc.request` / `cafe.jsonrpc.response` — RPC protocol
- `cafe.binary.*` — binary-store protocol
- `cafe.flow.*` — pipeline flow control
- `cafe.tool.*` — tool call protocol
- `cafe.error.*` — error protocol

**Not prefixed (data / agent-authored):**
- `chat.*`, `config.*`, `session.*`, `security.*`, `web.*`, `tool.*` (the `tool.call`/`tool.result` KEYS moved to `cafe.*`, but tool NAMES remain data)

**Consequences**:
- Agents can safely publish arbitrary annotations without colliding with platform semantics
- Bus and service code clearly indicates which annotations carry platform meaning
- Old flat keys (`transient`, `jsonrpc.request`, etc.) remain defined in `annotation.rs` for backward compat but are no longer referenced by any code
- Consumers were updated in a single pass (25 files, 99 insertions/77 deletions)
