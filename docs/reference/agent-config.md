# Agent config reference

An **agent** is a TOML file in `agents/` describing a pipeline of steps.
`cafe-agent-runtime` watches this directory (hot-reload) and instantiates a
pipeline for each agent.

This page is the complete field reference. For a hands-on walkthrough, see
[Tutorial 2: your first agent](../tutorials/your-first-agent.md).

---

## File layout

```toml
name = "default"
description = "Standard chat agent"
background = false
allows_reload = true
persists_state = true

[[steps]]
id = "trust-filter"
type = "trust-filter"
trigger = "user_message"

[[steps]]
id = "llm"
type = "llm"
trigger = "user_message"

[initial_chunk]
type = "null"

[initial_chunk.annotations]
"config.type" = "runtime"
"config.llm.system_prompt" = "You are a helpful assistant."
```

## Top-level fields

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | *(required)* | Unique identifier. Used as the session ID for background agents. |
| `description` | string | `""` | Human-readable purpose. |
| `background` | bool | `false` | If `true`, the agent auto-starts at boot. |
| `allows_reload` | bool | `true` | If `false`, hot-reload is skipped (for stateful agents). |
| `persists_state` | bool | `true` | If `false`, session history is not written to SQLite. |
| `schedule` | string | `None` | Cron expression; when set, the agent runs on `scheduler_tick`. |
| `rpc_timeout_secs` | u64 | `60` | Per-agent RPC timeout for pipeline steps. |
| `max_pipeline_depth` | u32 | `10` | Maximum `step_complete` chaining depth. |
| `ephemeral_keepalive_secs` | u64 | `None` | When set, the session auto-deletes after all subscribers disconnect (ephemeral session). |
| `ephemeral_count_role` | string | `None` | Only subscribers with this role count toward ephemeral lifetime; `None` = count all. |

## `[[steps]]` — pipeline steps

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Unique ID within the agent; referenced by `step_complete:<id>` triggers. |
| `type` | string | Evaluator type: `llm`, `tts`, `comfy`, `trust-filter`, `tool-detector`, `tool-executor`, `rot13`, `web-fetch`, `rss-fetch`, `stt`, `mcp`, … |
| `trigger` | string | Fires the step: `user_message`, `llm_complete`, `scheduler_tick`, `step_complete:<id>`. |
| `enabled_if` | string | Optional `config.<key>` annotation key; the step is skipped unless the resolved config value is `true`. |

Steps run in array order. A `step_complete:<id>` trigger lets later steps run
after an earlier one finishes.

### Built-in step types

Handled internally by `cafe-agent-runtime` (no external RPC):

| Type | Purpose |
|---|---|
| `trust-filter` | Gate untrusted (`security.requires-review`) chunks until explicitly trusted. |
| `role-annotator` | Annotate chunk roles. |
| `tool-detector` | Parse `<|tool_call|>` markers from LLM output into `cafe.tool.call` annotations. |
| `tool-executor` | Execute `cafe.tool.call` chunks (bus RPC tools); skips `provider: "mcp"` calls. |
| `mcp` | Declarative no-op signalling that external MCP tools should be available. |

### RPC step types

Every other step type dispatches a `<type>.invoke` JSON-RPC request on the bus;
the matching service (e.g. `cafe-llm`, `cafe-tts`, `cafe-rot13`) handles it and
publishes the response. See [How to: add a service](../how-to/add-a-service.md).

## `[initial_chunk]` — config seeding

A chunk published into the session when it is created. This is how an agent
seeds its runtime config and advertised tools.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `type` | string | `"text"` | Chunk content type: `text`, `null`, `binary`. |
| `content` | string | `""` | Text content (for `text` chunks). |
| `data` | string | `None` | Base64-encoded bytes (for `binary` chunks). |
| `mime_type` | string | `None` | MIME type for binary chunks. |
| `annotations` | table | `{}` | Annotations to attach; keys are quoted dot-paths. |

### Legacy flat style

Prefer the nested `[initial_chunk]` table above. The flat legacy keys
(`initial_chunk_content`, `initial_chunk_type`, `initial_chunk_data`,
`initial_chunk_mime_type`, `initial_chunk_annotations`) are still accepted; the
nested table takes precedence when both are present.

### Config annotations

Runtime config is carried in `config.*` annotations on `null` chunks with
`config.type = "runtime"`. `cafe-agent-runtime` merges these into the session
config (see `cafe-agent-runtime/src/config.rs`); later chunks win per key.
Common keys:

| Key | Meaning |
|---|---|
| `config.llm.system_prompt` | System prompt for the `llm` step. |
| `config.llm.temperature` | LLM temperature. |
| `config.llm.max_tokens` | Max response tokens. |
| `config.llm.model` | Model name. |
| `config.llm.backend` | Backend: `ollama`, `openai`, `kobold`. |
| `config.tts.profile` / `config.tts.engine` | TTS voice profile / engine. |
| `config.tts.enabled` | Gate for the `enabled_if` TTS step. |
| `config.rss.url` | Feed URL for `rss-fetch`. |
| `tools.available` | Array of tool definitions advertised to the LLM. |

## Tool definitions (`tools.available`)

Each entry describes a tool the LLM may call:

```toml
"tools.available" = [
  { name = "dice.roll", description = "Roll dice",
    parameters = { type = "object",
      properties = { count = { type = "integer" }, sides = { type = "integer" } },
      required = ["count", "sides"] },
    tool_type = "rpc" },
]
```

| Field | Meaning |
|---|---|
| `name` | Tool name, e.g. `dice.roll`. |
| `description` | Shown to the LLM. |
| `parameters` | JSON Schema describing the arguments. |
| `tool_type` | `"rpc"` (bus RPC, handled by `tool-executor`) or `"mcp"` (external MCP server). |
| `provider` | `"mcp"` for external-MCP tools; omitted for bus RPC tools. |

## Example agents

| Agent | Highlights |
|---|---|
| `agents/default.toml` | Minimal chat: trust-filter + llm. |
| `agents/dice-llm.toml` | LLM → tool-detector → tool-executor → LLM round-trip. |
| `agents/voice.toml` | Adds a `tts` step gated by `enabled_if = "config.tts.enabled"`. |
| `agents/rss-summarizer.toml` | Background cron agent (`schedule = "0 7 * * *"`, `background = true`). |

---

## Reference

- [Data model spec, §5 Agent](../spec-cafe.md#5-agent)
- [Tutorial 2: your first agent](../tutorials/your-first-agent.md)
- [Tutorial 3: a tool-calling agent](../tutorials/tool-calling-agent.md)
- [How to: connect via MCP](../how-to/connect-mcp.md) — `provider: "mcp"` tools
- [ADR-121: Evaluator Schema System](../adr-121-evaluator-schema-system.md)
