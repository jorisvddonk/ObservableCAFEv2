# Explanation

Background and reasoning behind ObservableCAFE's design. Read these when you want
to *understand* the system — not when you need to do a task (that's
[How-to guides](../how-to/)) or look something up (that's [Reference](../reference/)).

---

## Core concepts

Read this first:

- [Architecture](../architecture.md) — system overview, data flow for a user
  message, startup order, key concepts, IPC protocol, HTTP API, and the six
  design principles.
- [Data model spec](../spec-cafe.md) — the *contracts* behind the concepts
  (Chunk, Session, Agent, Evaluator).

The central ideas:

- **Chunks are immutable.** Nothing mutates a chunk; services produce new chunks
  with updated annotations. State is derived from history.
- **The bus is the only shared state.** Services never call each other directly;
  all coordination flows through chunk streams on `cafe-bus`.
- **History is the source of truth.** Session state is reconstructed by scanning
  chunk history, not stored separately.
- **Errors are out-of-band.** Errors go to an error stream, never the data stream.
- **Agents are pipelines.** A JS module (`manifest` + `async function main(cafe)`)
  wires evaluators into a data flow; the TOML form is frozen.

## Architecture Decision Records

Each ADR records a design decision, its context, its consequences, and the
alternatives considered. They are the project's institutional memory.

### Foundations

| ADR | Decision |
|---|---|
| [ADR-001](../adr-001-unix-socket-message-bus.md) | All services talk through a central Unix-socket message bus. |
| [ADR-002](../adr-002-event-sourced-chunk-model.md) | Chunks are immutable events; history is the source of truth. |
| [ADR-003](../adr-003-annotations-as-metadata.md) | Annotations (key-value metadata) on chunks instead of a rigid schema. |
| [ADR-004](../adr-004-session-scoped-routing.md) | Chunks are routed within sessions (input/output/error streams). |
| [ADR-005](../adr-005-transient-sole-persistence-gate.md) | Transient chunks are never persisted; persistence gate is sole. |
| [ADR-006](../adr-006-jsonrpc-over-bus-annotations.md) | JSON-RPC request/response flows over bus annotations. |

### Bus evolution

| ADR | Decision |
|---|---|
| [ADR-100](../adr-100-subscribe-filtered.md) | Subscribe with filters instead of blanket subscribe. |
| [ADR-101](../adr-101-connection-ids.md) | The bus assigns connection IDs for direct delivery. |
| [ADR-102](../adr-102-direct-to-chunks.md) | `direct_to` routes a chunk to one connection, not a broadcast. |
| [ADR-103](../adr-103-mutation-chunks.md) | Null chunks mutate another chunk's annotations (late-bound metadata). |
| [ADR-104](../adr-104-transient-chunk-retention.md) | Transient chunks can be served to late joiners within a retention window. |
| [ADR-105](../adr-105-session-restore-on-restart.md) | Sessions restore from store history on restart. |
| [ADR-106](../adr-106-subscribe-all-migration.md) | Migration to `subscribe_all`. |
| [ADR-117](../adr-117-session-tags.md) | Sessions carry tags for discovery/classification. |
| [ADR-116](../adr-116-ephemeral-sessions.md) | Sessions auto-delete when all subscribers leave. |

### Binary streaming

| ADR | Decision |
|---|---|
| [ADR-107](../adr-107-binary-streaming.md) | Binary assets streamed via a separate HTTP store, referenced by chunk. |
| [ADR-114](../adr-114-binary-upload-completion-event.md) | Explicit event when a binary upload completes. |
| [ADR-115](../adr-115-http-binary-ref-rejection.md) | HTTP API rejects direct binary refs. |

### Networking

| ADR | Decision |
|---|---|
| [ADR-118](../adr-118-iroh-transport.md) | iroh P2P QUIC transport for remote bus clients. |
| [ADR-119](../adr-119-binary-codec.md) | Pluggable wire codec; bincode feature for compact framing. |
| [ADR-120](../adr-120-iroh-allowlist.md) | Peer-ID allowlist to restrict who can connect over iroh. |

### Services & integrations

| ADR | Decision |
|---|---|
| [ADR-108](../adr-108-annotation-key-namespace.md) | `cafe.*` reserved for keys interpreted by the platform. |
| [ADR-109](../adr-109-web-fetch-service.md) | Web fetch as a bus service. |
| [ADR-110](../adr-110-dynamic-http-proxy.md) | Dynamic HTTP route registration over the bus. |
| [ADR-111](../adr-111-knowledgebase.md) | LanceDB vector knowledgebase as a bus service. |
| [ADR-112](../adr-112-mcp-bridge.md) | Expose bus tools over Model Context Protocol. |
| [ADR-113](../adr-113-websocket-bridge.md) | WebSocket bridge for the web UI. |
| [ADR-121](../adr-121-evaluator-schema-system.md) | Self-describing evaluator schemas announced on the bus. |
| [ADR-127](../adr-127-js-agent-runtime.md) | Pure-JS agents (`manifest` + `main(cafe)`) replace TOML `[[steps]]` pipelines. |
| [ADR-128](../adr-128-stt-agent-driven.md) | STT is agent-driven; `cafe-stt` no longer auto-transcribes uploads. |
| [ADR-129](../adr-129-cafe-rss-service.md) | RSS fetch/parse as its own evaluator (`cafe-rss`). |
| [ADR-130](../adr-130-doc-site-mdbook.md) | Documentation site built with mdBook, deployed to GitHub Pages. |
| [ADR-131](../adr-131-js-agent-fetch.md) | JS agents can make outbound HTTP calls via `cafe.fetch` / global `fetch`. |
| [ADR-132](../adr-132-agent-reload-signal.md) | Agent runtimes reload on an admin signal over the `_cafe_agents` control session. |

## Feature completeness

- [Feature matrix](../feature-matrix.md) — which layer each feature lives in and
  its implementation status across bus, SDK, services, and frontends.

## Design principles

From the [architecture page](../architecture.md#design-principles):

1. Chunks are immutable.
2. The bus is the only shared state.
3. History is the source of truth.
4. Errors are out-of-band.
5. All services handle SIGTERM gracefully (flush work, exit 0).
6. Log to stdout/stderr only.

---

## Conventions for writers

- New design decisions become an ADR (`docs/adr-NNN-title.md`), cross-linked here.
- Explanations should answer *why*, not *how*. Point readers to how-to guides and
  reference pages for the *how* and the *what*.
