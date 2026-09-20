# ObservableCAFE

ObservableCAFE is a **multi-agent LLM platform** built as a suite of small,
composable programs that talk to each other over a central message bus. There
is no monolith and no direct service-to-service calls: the LLM, speech, image
generation, retrieval, and every tool are separate processes that connect to
the same bus.

The design follows the **CAFE** model — **C**hunks, **A**nnotations,
**F**unctions (evaluators), **E**vents:

- Every event is an immutable **chunk** (text, binary, or null).
- Chunks carry **annotations** — dot-namespaced metadata like `chat.role` or
  `config.llm.model` — instead of a rigid schema.
- **Evaluators** are the functions: given a chunk plus the session history,
  they produce zero or more new chunks.
- **Agents** wire evaluators into a pipeline. Here an agent is a plain
  JavaScript module — `manifest` + `async function main(cafe)` — run by the
  `cafe-agent-js` host.

Nothing is stored out of band: a session *is* its ordered chunk history, and
every service derives what it needs by scanning that history. The services are
written in Rust (plus a Go bridge and TypeScript frontend), and the agent-facing
API is promises all the way down — `await cafe.invoke("llm", {})`.

## Quick start

```sh
git clone https://github.com/jorisvddonk/ObservableCAFEv2
cd ObservableCAFEv2
cargo build --workspace     # build the Rust services
./start.sh                  # start the bus, services and HTTP API
```

Then walk through **[Getting started](tutorials/getting-started.md)**, or jump
straight to writing an agent in **[Your first JS
agent](tutorials/your-first-js-agent.md)**.

## Key concepts

| Concept | What it is |
|---|---|
| **Bus** | `cafe-bus`, a Unix-socket message bus. Every service connects to it; nothing calls anything else directly. |
| **Chunk** | The unit of data — immutable, with a content type, a producer and annotations. |
| **Annotation** | Key/value metadata on a chunk, e.g. `chat.role`, `cafe.tool.call`, `config.*`. |
| **Session** | An ordered chunk history with input/output/error streams. State is derived from it. |
| **Evaluator** | A function that turns a chunk plus history into more chunks (`llm`, `tts`, `rot13`, …). |
| **Agent** | A JS module that orchestrates evaluators and tools for a session. |
| **Service** | A standalone process exposing one capability over the bus. |

## What you can build

- **Chat and tool-calling agents** — the LLM decides, a tool runs over RPC, and
  the result flows back into the answer ([JS tutorial](tutorials/js-tool-calling-agent.md)).
- **Voice agents** — speech in and out via `cafe-stt` and `cafe-tts`.
- **Image generation** — ComfyUI workflows driven by an agent (`cafe-comfy`).
- **Retrieval-augmented answers** — a vector knowledge base (`cafe-knowledgebase`).
- **Background and scheduled agents** — cron-driven JS agents that run on their own.
- **Remote buses** — connect clients over iroh QUIC from another machine.
- **AI-assistant integration** — expose the whole bus to opencode or Claude over MCP.

## Browse the docs

The documentation is split into four kinds, so you can go straight to the page
that matches what you need:

| Section | Use it when you want to… |
|---|---|
| [Tutorials](tutorials/) | learn by following a guided lesson — **start here** |
| [How-to guides](how-to/) | accomplish a specific task |
| [Reference](reference/) | look up an exact contract: data model, bus protocol, HTTP API, annotations |
| [Explanation](explanation/) | understand *why* — architecture and the ADR decision log |

New here? Read [Getting started](tutorials/getting-started.md), then
[Architecture](architecture.md) for the bigger picture.

---

Source, issues and the full project list are on
[GitHub](https://github.com/jorisvddonk/ObservableCAFEv2). Licensed MIT.
