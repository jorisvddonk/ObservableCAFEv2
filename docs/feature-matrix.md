# Feature matrix

Where each feature lives and what it requires.

## SubscribeFiltered

| Layer | What | Status |
|---|---|---|
| **cafe-types** | `SubscribeFilter` struct, `SubscribeFiltered` `ClientMessage` variant | Done |
| **cafe-bus** | `chunk_matches_filter()`, `session_matches_filter()`, `SubscribeFiltered` handler (snapshot + registry events + live forwarding) | Done |
| **cafe-sdk** | `subscribe_filtered(filter)` on `BusClient` | Done |
| **cafe-store** | Switched to `SubscribeFiltered { transient: false }` | Done |

Tests: `cafe-bus` unit tests for both matcher functions.

## Connection IDs

| Layer | What | Status |
|---|---|---|
| **cafe-types** | `Connected` `ServerMessage` variant, `SOURCE_CONNECTION` annotation key | Done |
| **cafe-bus** | `ConnectionRegistry` (shared `HashMap<String, Arc<Mutex<OwnedWriteHalf>>>`), atomic counter for IDs, register/unregister on connect/disconnect, inject `source.connection` annotation on every `Publish` | Done |
| **cafe-sdk** | `connect()` silently skips `Connected` message (backward compat) | Done |

No new SDK methods — connection IDs are an internal bus mechanism, exposed to SDK consumers only via chunk annotations (reading `source.connection` from received chunks).

Tests: none yet (integration-level — would need a running bus).

## Direct-to

| Layer | What | Status |
|---|---|---|
| **cafe-types** | `DIRECT_TO` annotation key | Done |
| **cafe-bus** | `Publish` handler checks `direct_to` annotation, routes to target connection writer via `ConnectionRegistry`, returns `TARGET_NOT_FOUND` error if target missing | Done |
| **cafe-sdk** | `publish_direct(target_connection, session_id, chunk)` — adds `direct_to` annotation + `as_transient()` | Done |

Tests: none yet (integration-level — needs running bus + two connections).

## Mutations

| Layer | What | Status |
|---|---|---|
| **cafe-types** | `MUTATES_TARGET_ID` annotation key, `is_mutation()` / `mutation()` on `Chunk` | Done |
| **cafe-sdk** | Re-exported from cafe-types | Done |
| **cafe-tui** | Chunk loop detects `mutates.target_id`, merges annotations into target chunk (excludes `mutates.target_id` key itself), discards mutation chunk | Done |
| **cafe-web** | `appendChunk` / `finaliseStream` detect `mutates.target_id`, merge into target in `allChunks` | Done |

**No bus changes required**: mutations are regular chunks with a specific annotation. Persistence follows the standard `transient` rule.

Tests: `cafe-types` unit tests for `is_mutation()` and `mutation()`.

## Transient retention

| Layer | What | Status |
|---|---|---|
| **cafe-types** | `TRANSIENT_RETAIN_SECS` annotation key, `retain_secs()` / `with_retain()` on `Chunk` | Done |
| **cafe-bus** | `retained: Vec<(Chunk, Instant)>` in `SessionState`, `drain_retained()` prunes expired entries, `publish()` stores retained transient chunks, `Subscribe`/`SubscribeFiltered` include retained before `HistoryComplete` | Done |
| **Multiple producers** | `executor.rs`, `tool_executor.rs`, `evaluator.rs`, `worker.rs` (tts) set `.with_retain(60)` on transient RPC chunks | Done |

Tests: `cafe-types` unit tests for `retain_secs()` / `with_retain()`.

## BinaryRef

| Layer | What | Status |
|---|---|---|
| **cafe-types** | `ContentType::BinaryRef` variant, annotation keys | Done |
| **cafe-binary-store** | Axum HTTP API, disk storage, JWT auth, GC | Done |
| **cafe-server** | `serialize_chunk` for BinaryRef SSE shape | Done |
| **cafe-sdk** | `publish_direct()`, `subscribe_filtered()` | Done |
| **cafe-tts** | Publish BinaryRef + `binary.byte_size` annotation | Done |
| **cafe-web-sdk / cafe-web** | `BinaryRefContent` type, mutation merging, `getBinaryUrl()` | Done |
| **cafe-stt** | Auto-transcribe BinaryRef chunks from binary-store | Done |
| **process-compose** | `cafe-binary-store` process | Done |
| **E2E tests** | `binary-ref-e2e.py` — full write/read flow | Done |

## Summary: feature boundaries

```
SDK-only (no bus changes):
  - Mutations (annotation + client-side merge)
  - Chunk helpers (is_mutation, mutation, retain_secs, with_retain)

Bus + SDK:
  - SubscribeFiltered (new message type + bus handler + SDK method)
  - Connection IDs (bus assigns + SDK skips Connected)
  - Direct-to (bus routes + SDK publish_direct)
  - Transient retention (bus SessionState + SDK chunk helpers)

New binary:
  - Binary-store (standalone, connects via subscribe_filtered)
```
