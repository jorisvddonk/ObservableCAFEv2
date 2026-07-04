# `cafe.*` annotation keys

Annotations prefixed with `cafe.` are interpreted by the bus or platform
services. All other annotations (`chat.*`, `config.*`, `session.*`,
`security.*`, `web.*`, etc.) are data — agents and services can freely
read/write them without bus-level meaning.

## Transport / Bus

| Key | Type | Added by | Meaning |
|---|---|---|---|
| `cafe.transient` | `bool` | Producer | Chunk is ephemeral — broadcast live but never persisted in history |
| `cafe.transient.retain_secs` | `u64` | Producer | Serve this transient chunk to late-joining subscribers within this window |
| `cafe.source.connection` | `string` | Bus (auto) | Connection ID of the publisher — enables direct_to replies |
| `cafe.direct_to` | `string` | Producer | Route this chunk exclusively to the given connection ID (no broadcast) |
| `cafe.mutates.target_id` | `string` | Producer | Mutation: merge this chunk's annotations into the chunk with this ID |

## JSON-RPC over Bus

| Key | Type | Added by | Meaning |
|---|---|---|---|
| `cafe.jsonrpc.request` | `JsonRpcRequest` | Pipeline / tool-executor | JSON-RPC 2.0 request dispatched to a service |
| `cafe.jsonrpc.response` | `JsonRpcResponse` | Handler | JSON-RPC 2.0 response published back to the pipeline |

## Binary Store

| Key | Type | Added by | Meaning |
|---|---|---|---|
| `cafe.binary.write_url` | `string` | Binary-store (direct_to mutation) | URL to POST bytes to |
| `cafe.binary.write_token` | `string` | Binary-store (direct_to mutation) | JWT write token for POST |
| `cafe.binary.read_url` | `string` | Binary-store (broadcast mutation) | URL to GET bytes from |
| `cafe.binary.read_token` | `string` | Binary-store (broadcast mutation) | JWT read token for GET (no expiry) |
| `cafe.binary.byte_size` | `u64` | Producer | Optional: expected file size in bytes |

## Pipeline / Flow

| Key | Type | Added by | Meaning |
|---|---|---|---|
| `cafe.flow.signal` | `string` | Producer | Flow control signal (`"reset"`, `"tick"`, `"delete"`) |
| `cafe.flow.agent_id` | `string` | Pipeline | Agent ID attached to initial config chunks |
| `cafe.flow.tombstone` | `Vec<String>` | cafe-llm | IDs of transient token chunks to remove from display |

## Tool Calls

| Key | Type | Added by | Meaning |
|---|---|---|---|
| `cafe.tool.call` | `ToolCall` | Tool-detector step | Structured tool call parsed from LLM output |
| `cafe.tool.result` | `ToolResult` | Tool-executor step | Result of a tool execution |

## Error

| Key | Type | Added by | Meaning |
|---|---|---|---|
| `cafe.error.message` | `string` | Any | Human-readable error description |
| `cafe.error.code` | `string` | Any | Machine-readable error code |
