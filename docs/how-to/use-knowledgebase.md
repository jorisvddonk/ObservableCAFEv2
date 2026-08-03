# How to: Set up the knowledge base

This guide shows you how to give the LLM retrieval over your own documents using
`cafe-knowledgebase` — a LanceDB-backed vector search service that lives on the
bus.

---

## The mental model

- **Namespace** — a named collection of documents (think "project" or "tenant").
- **Document** — one indexed file (a `doc_id`, optional JSON metadata).
- **Indexing** — a document is split into chunks and embedded; the embeddings go
  into a LanceDB store.
- **Search** — a semantic query returns the nearest chunks, optionally with
  neighboring chunks for context.

The service is a bus RPC server: `knowledgebase.index`, `knowledgebase.search`,
`knowledgebase.search_with_context`, `knowledgebase.list`, `knowledgebase.delete`.

## 1. Make sure it's running

`cafe-knowledgebase` starts with the stack (see [How to: run the stack](run-the-stack.md)).
It needs an embedding endpoint to work:

| Env var | Default | Meaning |
|---|---|---|
| `CAFE_KNOWLEDGEBASE_DB_PATH` | `./knowledgebase.lance` | LanceDB store |
| `CAFE_KNOWLEDGEBASE_EMBED_URL` | `http://localhost:6900/v1/embeddings` | Embedding HTTP endpoint |
| `CAFE_KNOWLEDGEBASE_EMBED_MODEL` | `user.gemma3-embed` | Embedding model |
| `CAFE_KNOWLEDGEBASE_EMBED_DIM` | `1152` | Embedding dimension |

Verify the model backend is up before indexing — if embeddings fail, indexing
will fail.

## 2. Index a document (CLI)

`cafe-knowledgebase-index` reads a file and calls the `index` RPC:

```sh
cafe-knowledgebase-index geography notes-on-sweden.md \
    --doc-id sweden-notes --metadata '{"topic":"geography"}' \
    --bus /tmp/cafe-bus.sock
```

Optional flags:

| Flag | Default | Meaning |
|---|---|---|
| `--doc-id` | auto-generated | Stable ID for the document |
| `--metadata` | — | JSON metadata attached to every chunk |
| `--chunk-size` | `512` | Chunk size in characters |
| `--chunk-overlap` | `64` | Chunk overlap in characters |
| `--bus` | `$CAFE_BUS_SOCKET` | Bus socket path |

## 3. Index programmatically (bus RPC)

Any service can index by publishing a `knowledgebase.index` JSON-RPC request on
the bus. See [ADR-111](../adr-111-knowledgebase.md) for the request/response
shape, and the MCP bridge exposes the same operation as `kb_index`.

## 4. Search

```sh
# Via the CLI? No dedicated command — use the MCP tool or the RPC directly.

# Via MCP (requires cafe-mcp-bridge):
#   kb_search(namespace="geography", query="capital city", k=3)
#   kb_search_context(namespace="geography", query="capital city", k=3, context_chunks=2)
```

`kb_search_context` returns each match with `context_chunks` neighbors on either
side — useful for feeding the LLM enough surrounding text to answer from.

## 5. List and delete

```sh
kb_list(namespace="geography")            # list documents in a namespace
kb_delete(namespace="geography", doc_id="sweden-notes")  # delete a document + its chunks
```

## 6. Give an agent retrieval

Wire a retrieval step into an agent pipeline so the LLM can query your docs. The
tool-based route (LLM calls `kb_search` itself) mirrors
[Tutorial 3: a tool-calling agent](../tutorials/tool-calling-agent.md) — add
`knowledgebase.search` to `tools.available` with `tool_type = "rpc"`.

---

## Reference

- [ADR-111: Knowledgebase](../adr-111-knowledgebase.md) — architecture, RPC shapes, chunking
- [`cafe-kb` MCP tools](../adr-112-mcp-bridge.md#tools)
- [How to: connect via MCP](connect-mcp.md)
