# Summary

# Home

- [Introduction](index.md)

# Tutorials

- [Tutorials](tutorials/index.md)
- [Tutorial 1: Getting started](tutorials/getting-started.md)
- [Tutorial 2: Your first agent](tutorials/your-first-agent.md)
- [Tutorial 3: A tool-calling agent](tutorials/tool-calling-agent.md)
- [Tutorial: Your first JS agent](tutorials/your-first-js-agent.md)
- [Tutorial: A JS tool-calling agent](tutorials/js-tool-calling-agent.md)

# How-to guides

- [How-to guides](how-to/index.md)
- [How to: Run the stack](how-to/run-the-stack.md)
- [How to: Use the cafe-cli](how-to/use-the-cli.md)
- [How to: Connect via MCP](how-to/connect-mcp.md)
- [How to: Work with binary assets](how-to/use-binary-assets.md)
- [How to: Set up the knowledge base](how-to/use-knowledgebase.md)
- [How to: Write and run E2E tests](how-to/write-e2e-tests.md)
- [How to: Connect over iroh](how-to/connect-over-iroh.md)
- [How to: Add a service](how-to/add-a-service.md)
- [How to: Debug a JS agent](how-to/debug-js-agent.md)

# Reference

- [Reference](reference/index.md)
- [JS agent reference](reference/js-agents.md)
- [Agent config reference (TOML, legacy)](reference/agent-config.md)
- [CAFE Data Model Specification](spec-cafe.md)
- [Bus Protocol Specification](spec-bus-protocol.md)
- [HTTP API Specification](spec-http-api.md)
- [`cafe.*` annotation keys](cafe-annotations.md)
- [cafe-comfy](cafe-comfy.md)

# Explanation

- [Explanation](explanation/index.md)
- [Architecture](architecture.md)
- [Feature matrix](feature-matrix.md)

# Architecture Decision Records

## Foundations

- [ADR-001: Unix socket message bus](adr-001-unix-socket-message-bus.md)
- [ADR-002: Event-sourced chunk model](adr-002-event-sourced-chunk-model.md)
- [ADR-003: Annotations as metadata](adr-003-annotations-as-metadata.md)
- [ADR-004: Session-scoped routing](adr-004-session-scoped-routing.md)
- [ADR-005: Transient annotation as sole persistence gate](adr-005-transient-sole-persistence-gate.md)
- [ADR-006: JSON-RPC over bus annotations](adr-006-jsonrpc-over-bus-annotations.md)

## Bus evolution

- [ADR-100: SubscribeFiltered — bus-side chunk filtering](adr-100-subscribe-filtered.md)
- [ADR-101: Connection IDs](adr-101-connection-ids.md)
- [ADR-102: Direct-to chunks](adr-102-direct-to-chunks.md)
- [ADR-103: Mutation chunks](adr-103-mutation-chunks.md)
- [ADR-104: Transient chunk retention](adr-104-transient-chunk-retention.md)
- [ADR-105: Session restore on bus restart](adr-105-session-restore-on-restart.md)
- [ADR-106: SubscribeAll migration](adr-106-subscribe-all-migration.md)
- [ADR-116: Ephemeral Sessions with Connection Roles](adr-116-ephemeral-sessions.md)
- [ADR-117: Session Tags](adr-117-session-tags.md)

## Binary streaming

- [ADR-107: Binary streaming via binary-ref chunks](adr-107-binary-streaming.md)
- [ADR-114: Binary Upload Completion Event + Auto-Transcription](adr-114-binary-upload-completion-event.md)
- [ADR-115: HTTP BinaryRef Publish Rejection](adr-115-http-binary-ref-rejection.md)

## Networking

- [ADR-118: iroh transport for remote bus connectivity](adr-118-iroh-transport.md)
- [ADR-119: Binary codec with protocol negotiation](adr-119-binary-codec.md)
- [ADR-120: iroh peer-ID allowlist for the bus](adr-120-iroh-allowlist.md)

## Services & integrations

- [ADR-108: Annotation key namespace (`cafe.*`)](adr-108-annotation-key-namespace.md)
- [ADR-109: Web fetch service](adr-109-web-fetch-service.md)
- [ADR-110: Dynamic HTTP Proxy over Bus](adr-110-dynamic-http-proxy.md)
- [ADR-111: Knowledgebase — Vector Search on the Bus](adr-111-knowledgebase.md)
- [ADR-112: MCP Bridge — Bus Tools over Model Context Protocol](adr-112-mcp-bridge.md)
- [ADR-113: WebSocket Bridge for HTTP Clients](adr-113-websocket-bridge.md)
- [ADR-121: Evaluator Schema System](adr-121-evaluator-schema-system.md)

## Documentation & tooling

- [ADR-130: Documentation site built with mdBook](adr-130-doc-site-mdbook.md)

## Agents, LLM & TTS

- [ADR-122: Per-session TTS backend selection](adr-122-per-session-tts-backend.md)
- [ADR-123: Session forking](adr-123-session-forking.md)
- [ADR-124: Chunk timestamps are authoritative and preserved on fork](adr-124-chunk-timestamps.md)
- [ADR-125: Configurable LLM history compaction](adr-125-llm-compaction.md)
- [ADR-126: Chunk long speech-server TTS requests](adr-126-tts-chunking.md)
- [ADR-127: JS Agent Runtime](adr-127-js-agent-runtime.md)
- [ADR-128: STT is agent-driven (no auto-transcription)](adr-128-stt-agent-driven.md)
- [ADR-129: RSS as its own evaluator (cafe-rss)](adr-129-cafe-rss-service.md)
