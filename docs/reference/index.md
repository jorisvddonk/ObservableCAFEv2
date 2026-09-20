# Reference

Complete, factual descriptions of every contract in ObservableCAFE. Use these
when you need an exact field name, message shape, or endpoint — not to learn how
to do something (that's [How-to guides](../how-to/)) or why something is the way
it is (that's [Explanation](../explanation/)).

## Specifications

| Document | What it covers |
|---|---|
| [Data model spec](../spec-cafe.md) | The `Chunk` type, `ContentType`, every standard annotation key, sessions, agents, mutation chunks, wire envelope |
| [Bus protocol spec](../spec-bus-protocol.md) | Every client→bus and bus→client message, replay behavior, error codes, iroh transport, wire codecs |
| [HTTP API spec](../spec-http-api.md) | Every REST + SSE endpoint of `cafe-server`, auth, SSE event format, admin API |
| [`cafe.*` annotation keys](../cafe-annotations.md) | Keys interpreted *by the bus and platform services* (transport, JSON-RPC, binary, flow, tools, errors) |
| [Feature matrix](../feature-matrix.md) | Where each feature lives across layers and its implementation status |

## Configuration

| Document | What it covers |
|---|---|
| [JS agent reference](js-agents.md) | The `agents-js/*.js` format: manifest, `main(cafe)`, events, promise RPC (current) |
| [Agent config reference](agent-config.md) | The legacy TOML format: metadata fields, steps, triggers, initial chunk, tool definitions (frozen) |
| [Environment variables](https://github.com/jorisvddonk/ObservableCAFEv2/blob/main/internal-docs/AGENT.md#environment-variables-global) | Every env var read by the services and its default |

## Interfaces

| Document | What it covers |
|---|---|
| [cafe-cli](../how-to/use-the-cli.md) | Every CLI command and flag |
| [cafe-comfy](../cafe-comfy.md) | ComfyUI image generation service specifics |
| [MCP tool tables](../adr-112-mcp-bridge.md#tools) | Every tool exposed by `cafe-mcp-bridge` and its RPC method |

---

> Reference pages state *what is true today*. If something behaves differently
> from a spec, the spec is the source of truth to fix — file it as a bug.
