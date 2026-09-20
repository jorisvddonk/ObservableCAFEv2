# Documentation

ObservableCAFE's documentation follows the [Diataxis](https://diataxis.fr/) framework.
Every document here serves exactly one purpose — learning, doing, lookup, or
understanding — and links to its neighbours so you never get stuck between
quadrants.

The four quadrants:

| Quadrant | Question it answers | Style | You |
|---|---|---|---|
| [Tutorials](tutorials/) | "Where do I start?" | Guided lessons | Follow along, step by step |
| [How-to guides](how-to/) | "How do I do X?" | Recipes for real tasks | Working toward a goal |
| [Reference](reference/) | "What is this exactly?" | Dry, complete facts | Looking something up |
| [Explanation](explanation/) | "Why is it built this way?" | Background and reasoning | Building mental models |

> A tutorial is where you *learn*; a how-to is where you *do*; reference is where
> you *check*; explanation is where you *understand*.

---

## Tutorials — start here

Learn by doing. Follow these in order; each one builds on the last.

1. [Getting started](tutorials/getting-started.md) — build the workspace, start
   the stack, and send your first message end to end.
2. [Your first agent](tutorials/your-first-agent.md) — write an agent definition
   that transforms text, no Rust required.
3. [A tool-calling agent](tutorials/tool-calling-agent.md) — wire the LLM to a
   tool and back, the full round-trip.
4. [Your first JS agent](tutorials/your-first-js-agent.md) — the same idea as
   (2) as a pure-JS agent: `manifest` + `async function main(cafe)`.
5. [A JS tool-calling agent](tutorials/js-tool-calling-agent.md) — the
   round-trip from (3) orchestrated in promises instead of TOML steps.

## How-to guides — do real tasks

Goal-oriented recipes. Pick whichever matches what you're trying to accomplish.

| Task | Guide |
|---|---|
| Start or stop all services | [Run the stack](how-to/run-the-stack.md) |
| Talk to the bus from a shell | [Use the cafe-cli](how-to/use-the-cli.md) |
| Give an AI assistant (opencode, Claude) access to the bus | [Connect via MCP](how-to/connect-mcp.md) |
| Store and stream binary assets (audio, images, files) | [Work with binary assets](how-to/use-binary-assets.md) |
| Give the LLM retrieval over your documents | [Set up the knowledge base](how-to/use-knowledgebase.md) |
| Verify the whole stack works | [Write and run E2E tests](how-to/write-e2e-tests.md) |
| Connect to the bus from another machine | [Connect over iroh](how-to/connect-over-iroh.md) |
| Add a brand-new service to the stack | [Add a service](how-to/add-a-service.md) |
| Debug a JS agent (logs, error chunks, hot-reload) | [Debug a JS agent](how-to/debug-js-agent.md) |

## Reference — look it up

Complete, factual descriptions of every contract in the system.

- [Reference index](reference/) — data model, bus protocol, HTTP API, annotations, agents.
- [Data model spec](spec-cafe.md) — Chunk, ContentType, annotation keys, sessions, agents.
- [Bus protocol spec](spec-bus-protocol.md) — every message over the wire, including iroh.
- [HTTP API spec](spec-http-api.md) — every REST + SSE endpoint of `cafe-server`.
- [`cafe.*` annotation keys](cafe-annotations.md) — keys interpreted by the bus and platform services.
- [JS agent reference](reference/js-agents.md) — the `agents-js/*.js` format: manifest, `main(cafe)`, events, promise RPC.
- [Agent config reference](reference/agent-config.md) — the legacy TOML format for agent definitions (frozen; new agents are JS).

## Explanation — understand the design

Why the system looks the way it does. Read these when you want the bigger picture,
not when you need to get something done.

- [Architecture](architecture.md) — system overview, data flow, design principles.
- [Feature matrix](feature-matrix.md) — where each feature lives and its status.
- [Architecture Decision Records](explanation/) — every ADR, from the Unix socket
  bus (ADR-001) to the evaluator schema system (ADR-121).

---

## Mapping: where the old docs went

Nothing was deleted or moved. The existing documentation now has a home in the
quadrant it always belonged to:

| Existing file | Quadrant | Why |
|---|---|---|
| `docs/spec-cafe.md` | Reference | Contract, looked up |
| `docs/spec-bus-protocol.md` | Reference | Contract, looked up |
| `docs/spec-http-api.md` | Reference | Contract, looked up |
| `docs/cafe-annotations.md` | Reference | Key table, looked up |
| `docs/architecture.md` | Explanation | Background and reasoning |
| `docs/feature-matrix.md` | Explanation | State of the design |
| `docs/adr-*.md` | Explanation | Decision history |
| `docs/AGENT.md` | Explanation (internal) | Context for AI coding agents |
| `docs/cafe-comfy.md` | Reference | Service-specific facts |

---

## Conventions

- New specs and ADRs follow the rules in [`../AGENTS.md`](../AGENTS.md): ADRs are
  `docs/adr-NNN-title.md`, specs are `docs/spec-*.md`, how-tos are
  `docs/how-to/`, tutorials are `docs/tutorials/`.
- Every how-to and tutorial links to the relevant reference and explanation pages;
  keep those cross-links when editing.
