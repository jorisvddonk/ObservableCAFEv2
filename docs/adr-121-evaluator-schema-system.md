# ADR-121: Evaluator Schema System

**Status**: Design complete, not yet implemented (commits to follow)

**Driver**: Need for dynamic evaluator registration and self-describing evaluator configuration

**Context**: Evaluators (step types like `llm`, `tts`, `comfy`) are hardcoded in
`cafe-agent-runtime` — `SessionConfig` has per-evaluator Option fields,
`build_rpc_params()` is a 17-arm match, and `apply_config_key()` is another
17-arm match. Adding a new evaluator requires modifying three files in
`cafe-agent-runtime` and restarting it. There is no way for bus subscribers
(TUI, CLI, Web, LLM tools) to discover what configuration keys an evaluator
supports.

Separately, the term "agent" (TOML files, `AgentDefinition`, etc.) conflates
the workflow definition with the runtime process.
The eventual workflow/agent renaming is tracked alongside [ADR-127](./adr-127-js-agent-runtime.md), which renames the runtime concepts in practice.

**Decision**:

### 1. Evaluator schema type

A new `EvaluatorSchema` struct in `cafe-types/src/schema.rs` describes each
evaluator's config keys and RPC-invoke parameters using JSON Schema (draft-07):

```rust
pub struct EvaluatorSchema {
    pub name: String,
    pub description: String,
    pub config_schema: serde_json::Value,
    pub rpc_params_schema: serde_json::Value,
}
```

- `name` matches the step `type` in agent TOML files (e.g. `"llm"`, `"tts"`, `"comfy"`).
- `config_schema` is a JSON Schema whose property keys use the full annotation
  key (e.g. `"config.llm.system_prompt"`).
- `rpc_params_schema` describes the parameters sent in the `{name}.invoke`
  RPC call.

### 2. Schema advertisement via well-known session

A dedicated `__schema__` session (constant in `cafe_types::schema::SCHEMA_SESSION`)
serves as a push-based registry. On startup, each evaluator service:

1. Connects to the shared bus (existing `run_with_reconnect` / `subscribe_all`).
2. Creates the `__schema__` session (idempotent — `SESSION_EXISTS` error is
   silently ignored).
3. Publishes a non-transient null chunk with annotation key `cafe.schema.evaluator`
   and value being the full `EvaluatorSchema` JSON.

`cafe-agent-runtime` subscribes to `__schema__` and collects all schema chunks
from history on startup, then listens for new chunks as evaluators start later.
The result is a `SchemaRegistry: HashMap<String, EvaluatorSchema>`.

### 3. Agent-runtime uses schemas, not hardcoded matches

The 17-arm `build_rpc_params()` and `apply_config_key()` matches are replaced
with schema-driven iteration. For each step in a pipeline:
- Look up the evaluator's `rpc_params_schema`.
- For each property, extract the matching value from the pipeline context.
- Unknown keys go into `SessionConfig.extra` (same as today).

This means adding a new evaluator requires **zero code changes** in
`cafe-agent-runtime` — just a new evaluator crate that implements
`schema()` and announces it on the bus.

### 4. Session-level schema discovery

When `cafe-agent-runtime` creates a session for an agent, it also publishes
the schemas for that agent's evaluators as null chunks in the session itself
(key: `cafe.schema.evaluator`). Any bus subscriber can read session history
to discover what configuration keys are available for that agent.

### 5. Why announce-on-bootup, not RPC

We considered using `schema.get` RPC (request-response) but chose push-based
announcement instead:

- **No target-session ambiguity**: RPC would need to be published on some
  session, but `schema.get` is not scoped to any conversation.
- **Order-independent**: The agent-runtime can start before or after evaluator
  services; schemas are discovered either way.
- **Self-healing**: `run_with_reconnect` retries until the bus is available;
  the evaluator re-announces its schema on each reconnect.
- **Simple to implement**: Each evaluator adds ~15 lines of code.

### 6. Renaming "agent" to "workflow"

Deferred. We keep "agent" in types and filenames for now. A future ADR will
cover the rename once there is a semantic reason (e.g., when agents contain
multiple workflows).

**Consequences**:

- Positive: Adding a new evaluator no longer requires touching
  `cafe-agent-runtime`. Just write a crate that announces its schema.
- Positive: Bus subscribers can discover evaluator capabilities dynamically.
- Positive: The `__schema__` session is a natural pub/sub registry that could
  be extended for runtime health/status later.
- Negative: Each evaluator crate holds a JSON Schema constant (~10-40 lines).
- Negative: JSON Schema is heavier than a custom struct but standard and
  interoperable.
- Negative: `cafe-agent-runtime` becomes dependent on evaluators being online
  for schema resolution at startup (gracefully degraded — falls back to
  `"type": "object", "properties": {}` on missing schema).

**Alternatives considered**:
- **RPC `schema.get`**: Rejected due to target-session ambiguity.
- **File-based schemas (TOML/JSON files in a directory)**: Rejected because
  it's static and doesn't allow runtime evaluator registration.
- **Plugin system with shared library loading**: Too complex for the current
  need; revisit for DAG workflows.
