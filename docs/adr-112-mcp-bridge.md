# ADR-112: MCP Bridge — Bus Tools over Model Context Protocol

**Status**: Implemented

## Context

AI assistants (opencode, Claude Desktop, Continue.dev) need to interact with
the cafe bus — search knowledge bases, transcribe audio, fetch web content,
manage sessions. The Model Context Protocol (MCP) provides a standard way for
these clients to discover and call tools.

## Decision

Create `cafe-mcp-bridge`, a standalone binary that translates MCP
`tools/call` requests into bus RPCs or inline operations. Supports two
transports: stdio (for opencode) and HTTP+SSE (for persistent deployment).

## Architecture

```
AI Assistant (MCP client)
    │  stdio or HTTP/SSE (JSON-RPC 2.0)
    ▼
cafe-mcp-bridge
    ├── inline handlers (web_fetch, cafe_meta_*)
    └── bus RPC dispatch (kb_*, stt_*, tts_*, dice_*, sheetbot_*, comfy_*)
            │
            ▼
         cafe-bus → cafe-knowledgebase / cafe-stt / cafe-tts / etc.
```

## Tools

### Service tools (always available)

| MCP name | RPC method | Description |
|---|---|---|
| `kb_search` | `knowledgebase.search` | Semantic search |
| `kb_search_context` | `knowledgebase.search_with_context` | Search + neighbors |
| `kb_index` | `knowledgebase.index` | Index a document |
| `kb_list` | `knowledgebase.list` | List docs |
| `kb_delete` | `knowledgebase.delete` | Delete doc |
| `stt_transcribe` | `stt.invoke` | Transcribe audio |
| `tts_synthesize` | `tts.invoke` | Synthesize speech |
| `dice_roll` | `dice.roll` | Roll dice |
| `web_fetch` | inline (reqwest) | Fetch URL + strip HTML |
| `sheetbot_list_tasks` | `sheetbot.list_tasks` | List tasks |
| `sheetbot_get_task` | `sheetbot.get_task` | Get task |
| `sheetbot_create_task` | `sheetbot.create_task` | Create task |
| `sheetbot_update_task` | `sheetbot.update_task` | Update task |
| `sheetbot_accept_task` | `sheetbot.accept_task` | Accept task |
| `sheetbot_complete_task` | `sheetbot.complete_task` | Complete task |
| `sheetbot_fail_task` | `sheetbot.fail_task` | Fail task |
| `sheetbot_delete_task` | `sheetbot.delete_task` | Delete task |
| `sheetbot_clone_task` | `sheetbot.clone_task` | Clone task |
| `sheetbot_get_next_task` | `sheetbot.get_next_task` | Get next task |
| `sheetbot_list_sheets` | `sheetbot.list_sheets` | List sheets |
| `sheetbot_get_sheet` | `sheetbot.get_sheet` | Get sheet |
| `sheetbot_upsert_sheet_data` | `sheetbot.upsert_sheet_data` | Upsert row |
| `sheetbot_delete_sheet_row` | `sheetbot.delete_sheet_row` | Delete row |
| `sheetbot_list_library` | `sheetbot.list_library` | List library |
| `comfy_generate` | `comfy.invoke` | Generate image |

### Meta tools (--meta flag)

| MCP name | Description |
|---|---|
| `cafe_meta_ping` | Ping the bus |
| `cafe_meta_list_sessions` | List all sessions |
| `cafe_meta_get_history` | Get session chunk history |
| `cafe_meta_publish_chunk` | Publish a text chunk |
| `cafe_meta_delete_session` | Delete a session |
| `cafe_meta_list_agents` | List agent configs |
| `cafe_meta_list_models` | List LLM models |

## Usage

### Stdio transport (opencode)

```json
{
  "mcpServers": {
    "cafe": {
      "command": "/path/to/target/release/cafe-mcp-bridge",
      "args": ["--tool", "kb_*", "--tool", "stt_transcribe", "--tool", "web_fetch"]
    }
  }
}
```

### HTTP transport (persistent)

```
# Start server (port 3100, meta tools enabled)
cafe-mcp-bridge --transport http --port 3100 --meta

# Connect with all tools
GET /sse

# Connect with only specific tools
GET /sse?tool=kb_search&tool=stt_transcribe

# Connect with meta tools only
GET /sse?tool=cafe_meta_*

# Send MCP request
POST /message?sessionId=<from_sse>
Content-Type: application/json

{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kb_search","arguments":{"namespace":"geography","query":"Sweden","k":3}}}
```

### CLI flags

| Flag | Default | Description |
|---|---|---|
| `--bus` | `/tmp/cafe-bus.sock` | Bus socket path |
| `--tool` | `*` (all) | Tool name glob patterns (repeatable) |
| `--transport` | `stdio` | `stdio` or `http` |
| `--port` | `3100` | HTTP port |
| `--meta` | `false` | Enable `cafe_meta_*` admin tools |

## Tool filtering

- **Stdio**: `--tool` CLI args applied at startup.
- **HTTP**: `?tool=<pattern>` query params on the SSE endpoint. Single or repeatable (`?tool=kb_search&tool=stt_transcribe`).
- Glob patterns: `*` matches any chars, `?` matches single char.
- When multiple `--tool` flags or query params are given, a tool is included if it matches ANY pattern.

## Bus protocol

For RPC-based tools, cafe-mcp-bridge:
1. Creates a temporary bus session (`_cafe_mcp_<uuid>`)
2. Subscribes and drains history
3. Publishes a `cafe.jsonrpc.request` chunk
4. Waits for matching `cafe.jsonrpc.response` (60s timeout)
5. Deletes the temporary session
6. Returns the result as MCP tool output

## Tests

| Test | File | What it covers |
|---|---|---|
| MCP Bridge | `tests/mcp-bridge-e2e.py` | Stdio transport: `tools/list`, `web_fetch` (inline), `cafe_meta_ping` (meta), `kb_search` (bus RPC) |
| MCP Client | `tests/mcp-client-e2e.py` | Fake MCP server → `tool.call` with `provider:mcp` → cafe-mcp-client → MCP server → `tool.result` round-trip |

Both tests use temporary bus instances (no process-compose dependency, no shared state).

## cafe-mcp-client — MCP Client on the Bus

### Architecture

```
LLM emits <|tool_call|>{"name":"tavily_search","provider":"mcp","parameters":{...}}
  │
  ▼
tool-detector → publishes tool.call { provider: "mcp" }
  │
  ├── tool-executor → skips (provider is "mcp")
  │
  └── cafe-mcp-client → forwards to external MCP server via stdio
          │
          ▼
        MCP server (tavily, filesystem, etc.)
          │
          ▼
        cafe-mcp-client → publishes tool.result { provider: "mcp" }
          │
          ▼
        tool-executor picks up tool.result → agent continues
```

### Provider field

All `ToolCall`, `ToolResult`, and `ToolDefinition` structs in `cafe-types`
now carry an optional `provider` field:

- `None` (or omitted) → bus RPC tool (handled by tool-executor)
- `Some("mcp")` → MCP tool (handled by cafe-mcp-client)

The field is `#[serde(skip_serializing_if = "Option::is_none")]` so existing
serialized data is backward compatible.

### Agent TOML

The `type = "mcp"` step is purely declarative — it's a no-op built-in step
that causes no RPC dispatch. It tells cafe-mcp-client (via session
subscription) and the pipeline that MCP tools should be available for this
agent.

```toml
name = "research"

[[steps]]
id = "llm"
type = "llm"
trigger = "user_message"

[[steps]]
id = "tool-detector"
type = "tool-detector"
trigger = "llm_complete"

[[steps]]
id = "tool-executor"
type = "tool-executor"
trigger = "step_complete:tool-detector"

[[steps]]
id = "mcp"
type = "mcp"
trigger = "user_message"
```

The MCP tool definitions must be listed in `tools.available` annotations
(with `provider: "mcp"`) so the LLM knows about them and includes the
provider in its tool calls.

### MCP Server Configuration: `mcp-servers.toml`

```toml
[[server]]
name = "tavily"
command = "npx"
args = ["-y", "@tavily/mcp"]

[[server]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
```

On startup, cafe-mcp-client:
1. Reads `mcp-servers.toml` (path via `CAFE_MCP_SERVERS` env var)
2. Spawns each server as a child process (stdio transport)
3. Sends `initialize` + reads response
4. Sends `tools/list` + discovers available tools
5. Registers tool→server mapping

On each session:
1. Subscribes via SubscribeAll
2. Reads `tool.call` chunks — only processes those with `provider: "mcp"`
3. Looks up the tool name in the registry
4. Forwards the call to the correct MCP server via JSON-RPC
5. Publishes `tool.result` + assistant text chunk back to the session

### Collision avoidance

| Tool type | provider field | Handler | Notes |
|---|---|---|---|
| Bus RPC | `None` (absent) | `tool-executor` → bus RPC | dice.roll, kb_search, etc. |
| MCP | `"mcp"` | `cafe-mcp-client` → external server | tavily_search, etc. |

Both handlers subscribe to the same session. They inspect the `provider`
field to decide whether to process or skip — no collisions.

## Files

| File | Purpose |
|---|---|
| `cafe-mcp-bridge/src/main.rs` | Entry, CLI, MCP dispatch, inline handlers, RPC dispatch |
| `cafe-mcp-bridge/src/tools.rs` | Tool definitions, schemas, filtering |
| `cafe-mcp-bridge/src/transport/stdio.rs` | stdin/stdout JSON-RPC loop |
| `cafe-mcp-bridge/src/transport/http.rs` | HTTP+SSE transport with per-session tool filtering |
| `cafe-mcp-client/src/main.rs` | MCP client: spawn servers, intercept tool.call, forward via JSON-RPC |
| `cafe-types/src/tools.rs` | `ToolCall`, `ToolResult`, `ToolDefinition` with `provider` field |
| `cafe-agent-runtime/src/executor.rs` | `"mcp"` step type (no-op built-in) |
| `cafe-agent-runtime/src/tool_executor.rs` | Skips `provider: "mcp"` calls |
| `mcp-servers.toml` | MCP server configuration |
| `process-compose.yml` | HTTP bridge on port 3100 with `--meta`, plus cafe-mcp-client |
