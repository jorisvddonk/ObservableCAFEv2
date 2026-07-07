# CAFE Data Model Specification

Version 2.2 (adapted from the original ObservableCAFE spec for this Rust implementation)

This document defines the core data model. It is the contract between all components.
`cafe-types` is the Rust implementation of this spec.

---

## 1. Chunk

A **Chunk** is the fundamental unit of data in the system. It is immutable.

### Fields

| Field         | Type                        | Required | Description                                      |
|---------------|-----------------------------|----------|--------------------------------------------------|
| `id`          | UUID v4 (string)            | yes      | Globally unique identifier                       |
| `content_type`| `ContentType` enum          | yes      | One of `text`, `binary`, `binary-ref`, `null`    |
| `content`     | `Option<String>`            | yes      | UTF-8 text if `text`; absent if `binary`/`null`  |
| `data`        | `Option<Vec<u8>>`           | yes      | Raw bytes if `binary`; absent otherwise          |
| `mime_type`   | `Option<String>`            | no       | MIME type for binary chunks (e.g. `image/png`)   |
| `producer`    | String (reverse-DNS)        | yes      | Source identifier, e.g. `com.nominal.cafe-llm`   |
| `annotations` | `HashMap<String, JsonValue>`| yes      | Key-value metadata; see annotation keys below    |
| `timestamp`   | i64 (Unix ms)               | yes      | Creation time in milliseconds since epoch        |

### ContentType enum

```
text       — UTF-8 string content
binary     — raw bytes (image, audio, file), base64-encoded in JSON
binary-ref — binary asset announced by reference; bytes stored in cafe-binary-store
null       — no content; used for signals, config, flow control
```

For `binary-ref` chunks, the `content` field contains a JSON object (overriding the
usual `string` value) with the referenced binary's metadata:

```json
{
  "chunk_id": "<uuid of the binary chunk>",
  "mime_type": "audio/wav",
  "byte_size": 123456
}
```

### JSON wire format

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "content_type": "text",
  "content": "Hello, world!",
  "data": null,
  "mime_type": null,
  "producer": "com.nominal.cafe-server",
  "annotations": {
    "chat.role": "user"
  },
  "timestamp": 1717123456789
}
```

Binary chunks encode `data` as base64 in JSON:
```json
{
  "id": "...",
  "content_type": "binary",
  "content": null,
  "data": "iVBORw0KGgo=",
  "mime_type": "image/png",
  "producer": "com.nominal.cafe-agent.image-gen",
  "annotations": {},
  "timestamp": 1717123456789
}
```

---

## 2. Standard Annotation Keys

Annotation keys use dot-namespaced strings. All values are JSON-typed.

### Chat / conversation

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `chat.role`               | string     | `user`, `assistant`, `system`                    |
| `chat.model`              | string     | LLM model that produced this chunk               |
| `chat.finish_reason`      | string     | `stop`, `length`, `tool_use`                     |
| `chat.token_count`        | number     | Token count for this chunk                       |
| `chat.is_streaming`       | bool       | True while chunk is being streamed               |
| `chat.stream_complete`    | bool       | True on the final chunk of a streamed response   |

### Session

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `session.id`              | string     | Session this chunk belongs to                    |
| `session.name`            | string     | Renames the session when present                 |

### Security / trust

| Key                          | Value type | Description                                   |
|------------------------------|------------|-----------------------------------------------|
| `security.trust-level`       | object     | `{ trusted: bool, source: string }`           |
| `security.requires-review`   | bool       | User must explicitly trust before LLM sees it |

### Configuration (null chunks only)

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `config.type`             | string     | `runtime` — marks this as a config chunk         |
| `config.backend`          | string     | LLM backend: `ollama`, `openai`, `kobold`        |
| `config.model`            | string     | Model name                                       |
| `config.system_prompt`    | string     | System prompt for this session                   |
| `config.temperature`      | number     | LLM temperature                                  |
| `config.max_tokens`       | number     | Max tokens for LLM response                      |

### Web content

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `web.source_url`          | string     | URL this content was fetched from                |
| `web.content_type`        | string     | HTTP content-type of the source                  |
| `web.fetch_time`          | number     | Unix ms when fetched                             |
| `web.error`               | bool       | True if fetch failed                             |

### Tool use

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `tool.call`               | object     | `{ name: string, arguments: object }`            |
| `tool.result`             | object     | `{ name: string, output: any }`                  |

### Binary assets

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `binary.write_url`        | string     | URL to POST binary data to (cafe-binary-store)   |
| `binary.write_token`      | string     | Write JWT for binary upload                      |
| `binary.read_url`         | string     | URL to GET binary data from (cafe-binary-store)  |
| `binary.read_token`       | string     | Read JWT for binary download                     |
| `binary.byte_size`        | number     | Total byte size of the binary asset              |

### Mutations (chunk metadata)

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `cafe.mutates`            | object     | `{ target_id: string }` — this chunk modifies the target chunk's annotations |
| `cafe.direct_to`          | string     | Connection ID; only that connection receives this chunk |

### Flow control (null chunks)

| Key                       | Value type | Description                                      |
|---------------------------|------------|--------------------------------------------------|
| `flow.signal`             | string     | `abort`, `reset`, `ping`                         |
| `flow.agent_id`           | string     | Target agent for this signal                     |

---

## 3. Evaluator

An **evaluator** is an async function (or async generator in the original JS) that takes
a chunk and produces zero or more chunks. In Rust, the natural representation is:

```rust
// Conceptual signature — actual trait in cafe-types
trait Evaluator: Send + Sync {
    fn evaluate(
        &self,
        chunk: Chunk,
        history: &[Chunk],
    ) -> BoxStream<'static, Result<Chunk, EvaluatorError>>;
}
```

Evaluators come in two kinds:
- **LLM evaluators** — call an LLM backend, stream back response chunks
- **Transform evaluators** — pure functions that annotate, filter, or reshape chunks

---

## 4. Session

A **session** is a named, ordered sequence of chunks with three streams:

- `input_stream` — chunks entering the session (user messages, uploads, signals)
- `output_stream` — chunks produced by agents (LLM responses, evaluator outputs)
- `error_stream` — errors that occur during processing

Sessions have a string ID. Background agents use their agent name as the session ID.

The full ordered history of `output_stream` chunks is the session's state. To reconstruct
current configuration, scan history in reverse for the most recent `config.type = runtime`
null chunk.

---

## 5. Agent

An agent is a pipeline builder. It receives a session context at initialization and
wires up a data flow from `input_stream` → evaluators → `output_stream`.

Agent definition (interface):

```
name        string   — unique identifier; used as session ID for background agents
description string   — human-readable purpose
background  bool     — if true, auto-starts on server boot
allows_reload bool   — if false, hot-reload is skipped (for stateful agents)
persists_state bool  — if false, history is not saved to SQLite
```

---

## 6. Mutation Chunks

A **mutation chunk** is a null chunk that modifies the annotations of another chunk
in-place. This enables adding late-bound metadata (e.g., write/read credentials for
binary assets) without sacrificing immutability of the original chunk.

```json
{
  "id": "mut-001",
  "content_type": "null",
  "content": null,
  "producer": "com.nominal.cafe-binary-store",
  "annotations": {
    "cafe.mutates": { "target_id": "orig-chunk-uuid" },
    "binary.write_url": "http://...",
    "binary.write_token": "eyJ..."
  }
}
```

The client library merges mutation annotations into the target chunk's annotation map.
Mutations arrive either via `cafe.direct_to` (private delivery to the publishing
connection) or broadcast (public, all subscribers).

---

## 7. Chunk envelope (bus wire format)

On the bus, chunks are wrapped in an envelope that carries routing information.
See `docs/spec-bus-protocol.md` for the full protocol.

```json
{
  "op": "publish",
  "session_id": "my-session",
  "chunk": { ...chunk fields... }
}
```
