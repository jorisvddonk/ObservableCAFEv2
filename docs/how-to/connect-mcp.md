# How to: Connect via MCP

This guide shows you how to expose the cafe bus to AI assistants (opencode,
Claude Desktop, Continue.dev) over the [Model Context Protocol](https://modelcontextprotocol.io),
and how to let agents on the bus call *external* MCP servers.

There are two directions:

1. **`cafe-mcp-bridge`** — make bus tools available *to* an MCP client (outbound to you).
2. **`cafe-mcp-client`** — make external MCP tools available *on* the bus (inbound for agents).

---

## 1. Expose bus tools to an AI assistant

`cafe-mcp-bridge` translates MCP `tools/call` requests into bus RPCs (or inline
operations). Full tool list: [ADR-112](../adr-112-mcp-bridge.md#tools).

### Stdio transport (for opencode, Claude Desktop)

Point the MCP client at the bridge binary:

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

Without `--tool` filters, *all* tools are exposed. Add `--meta` to include the
`cafe_meta_*` admin tools (ping, list sessions, publish chunks, …).

### HTTP transport (persistent, default port 3100)

The stack already runs the bridge in HTTP mode (`--transport http --port 3100 --meta`).

```sh
# Connect with all tools
GET http://localhost:3100/sse

# Connect with only specific tools
GET http://localhost:3100/sse?tool=kb_search&tool=stt_transcribe

# Send a tool call (sessionId comes from the SSE stream)
POST http://localhost:3100/message?sessionId=<from_sse>
Content-Type: application/json

{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kb_search","arguments":{"namespace":"geography","query":"Sweden","k":3}}}
```

### Tool filtering

- **Stdio**: `--tool <glob>` flags at startup (repeatable).
- **HTTP**: `?tool=<glob>` query params on the SSE endpoint (repeatable).
- Glob patterns: `*` matches any run of chars, `?` matches one.
- A tool is included if it matches **any** pattern.

### CLI flags

| Flag | Default | Description |
|---|---|---|
| `--bus` | `/tmp/cafe-bus.sock` | Bus socket path |
| `--tool` | `*` (all) | Tool name glob patterns (repeatable) |
| `--transport` | `stdio` | `stdio` or `http` |
| `--port` | `3100` | HTTP port |
| `--meta` | `false` | Enable `cafe_meta_*` admin tools |

## 2. Expose external MCP servers to the bus

If you want agents to call tools from an external MCP server (Tavily search,
filesystem access, etc.), `cafe-mcp-client` connects to those servers and
publishes their tools onto the bus.

### Configure the servers

`mcp-servers.toml` (path overridable via `CAFE_MCP_SERVERS`):

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

### Make the tools available to an agent

Add a declarative `mcp` step and list the external tools with `provider = "mcp"`
in the agent's `tools.available`:

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

[initial_chunk]
type = "null"

[initial_chunk.annotations]
"config.type" = "runtime"
"tools.available" = [
  { name = "tavily_search", description = "Web search via Tavily",
    parameters = { type = "object", properties = { query = { type = "string" } } },
    provider = "mcp", tool_type = "mcp" },
]
```

### How routing avoids collisions

Both `tool-executor` (bus RPC) and `cafe-mcp-client` (external) subscribe to the
same session. They look at the `provider` field on the `ToolCall`:

| `provider` | Handler |
|---|---|
| absent / `None` | `tool-executor` → bus RPC (e.g. `dice.roll`) |
| `"mcp"` | `cafe-mcp-client` → external MCP server |

The LLM emits a tool call with the provider included; the right handler picks it
up and publishes the `tool.result` back to the session.

---

## Reference

- [ADR-112: MCP Bridge](../adr-112-mcp-bridge.md) — full tool tables, bus protocol behind RPC tools
- [Agent config reference](../reference/agent-config.md) — `provider` and `tool_type` fields
- [How to: use the cafe-cli](use-the-cli.md) — the same tools without MCP
