# ADR-111: Knowledgebase — Vector Search on the Bus

**Status**: Implemented

## Context

The project needed a vector search capability for retrieval-augmented generation
(RAG). Bus services and pipeline agents need to embed text, index documents, and
search for relevant content by semantic similarity — all over the bus.

## Decision

Create two new artifacts:

| Artifact | Purpose |
|---|---|
| `cafe-knowledgebase` | Bus-connected service: embed, index, search, delete, list |
| `cafe-knowledgebase-index` | Standalone CLI for batch indexing |

### Architecture

```
cafe-knowledgebase (bus service)
    ↓
LanceDB (embedded vector DB, one directory per database)
    ↓
local filesystem (.lance directory, configurable)
```

LanceDB was chosen because:
- Embedded library — no separate server process
- Rust-native SDK — fits the project's tech stack
- Full metadata support alongside vectors (namespace, doc_id, text, metadata)
- ANN search with HNSW/IVF-PQ indexes
- Local file persistence — one directory per database
- Actively maintained (10.8k stars)

### Schema

Each namespace is a separate LanceDB table with the following schema:

```arrow
doc_id:     Utf8 (primary key)
text:       Utf8
metadata:   Utf8 (JSON blob, optional)
created_at: Utf8 (Unix timestamp string)
embedding:  FixedSizeList<Float32, dim>
```

### RPC Protocol

All methods are dispatched as JSON-RPC chunks on individual session
subscriptions (same pattern as cafe-dice).

| Method | Params | Returns |
|---|---|---|
| `knowledgebase.embed` | `{ text }` | `{ embedding: [f32] }` |
| `knowledgebase.index` | `{ namespace, doc_id?, text, metadata? }` | `{ doc_id }` |
| `knowledgebase.search` | `{ namespace, query, k? }` | `{ results: [{ doc_id, text, metadata, score }] }` |
| `knowledgebase.delete` | `{ namespace, doc_id }` | `{ deleted: true }` |
| `knowledgebase.list` | `{ namespace }` | `{ documents: [...] }` |

### Embedding API

Configurable via `CAFE_KNOWLEDGEBASE_EMBED_URL` and
`CAFE_KNOWLEDGEBASE_EMBED_MODEL`. Supports both Ollama
(`/api/embed`) and OpenAI (`/v1/embeddings`) formats.

### CLI

```
cafe-knowledgebase-index <namespace> <file>
    [--doc-id <id>] [--metadata <json>] [--bus <path>]
```

Reads file, generates embedding via API, publishes `knowledgebase.index`
RPC over the bus.

### Pipeline Integration

The `agents/knowledgebase.toml` agent wires a `knowledgebase-search` RPC step
to an LLM step:

```
user_message → knowledgebase-search RPC → llm → response
```

The `knowledgebase-search` step type dispatches `knowledgebase.search` RPC
and feeds results as context to the LLM.

### Configuration

| Env Var | Default | Description |
|---|---|---|
| `CAFE_KNOWLEDGEBASE_EMBED_URL` | `http://localhost:11434/api/embed` | Embedding API endpoint |
| `CAFE_KNOWLEDGEBASE_EMBED_MODEL` | `nomic-embed-text` | Embedding model name |
| `CAFE_KNOWLEDGEBASE_DB_PATH` | `./knowledgebase.lance` | LanceDB directory path |
| `CAFE_KNOWLEDGEBASE_EMBED_DIM` | `768` | Expected embedding dimension |

### Consequences

- Vector search is available to any bus-connected service via RPC
- Namespaces provide tenant isolation — each namespace is a separate table
- LanceDB handles ANN indexing and search automatically
- No separate vector DB server to manage
- Embedding API is configurable — works with local Ollama or remote OpenAI
- Indexing is straightforward via CLI or direct RPC

### Files Created

| File | Purpose |
|---|---|
| `cafe-knowledgebase/Cargo.toml` | Crate manifest |
| `cafe-knowledgebase/src/main.rs` | Bus service entry point + RPC dispatch |
| `cafe-knowledgebase/src/embed.rs` | Embedding API client |
| `cafe-knowledgebase/src/index.rs` | LanceDB CRUD operations |
| `cafe-knowledgebase-index/Cargo.toml` | CLI manifest |
| `cafe-knowledgebase-index/src/main.rs` | CLI for batch indexing |
| `agents/knowledgebase.toml` | RAG pipeline agent |
| `tests/fixtures/knowledgebase/` | Sweden test documents for e2e tests |

### Test Fixtures

Three documents about Sweden for e2e testing:
- `sweden-geography.txt` — landscape, climate, archipelagos
- `sweden-provinces.txt` — provinces by region, cultural highlights
- `sweden-cities.txt` — major cities and what they are known for
