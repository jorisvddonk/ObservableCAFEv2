# ADR-137: Web chat renders one streaming bubble, with tombstone/delete hiding

**Status**: Implemented

**Date**: 2026-10-01

**Driver**: The web chat rendered each streaming token as its own bubble, and
showed the durable final response in addition to the streamed text (duplicate).
Two independent causes:

1. cafe-llm publishes **transient per-token deltas** (`chat.is_streaming`,
   `cafe.transient`, no `chat.model`) followed by a durable full
   `response_chunk` (has `chat.model`) and a `stream_complete`, then a
   `cafe.flow.tombstone` naming the token ids.
2. The chat SSE (`/chat`) and the persistent session stream (`/stream`) deliver
   the **same chunk** to the React component, so tokens were accumulated twice
   (e.g. `DoDoing…`).

Additionally, "Raw" mode rendered the unfiltered chunk list, so every token
became a row.

**Context**: The renderer must (a) accumulate tokens into a single stable
element, (b) never double-count dual-channel delivery, (c) reconcile the streamed
text with the durable final response, and (d) honor tombstones and
[delete signals](./adr-135-chunk-deletion.md) without depending on the producer
to mutate history.

**Decision**:

1. **One bubble, stable key.** `nextLiveStream(prev, chunk)` accumulates deltas
   into a single `liveStream { key, content }`, keyed to the `stream_start`
   chunk id (or a sentinel), so React reuses one DOM node for the whole stream.
2. **Deduplicate dual-channel delivery by chunk id.** Both the chat SSE and the
   persistent stream call one `ingestLive` guarded by a `liveSeenIds` set, so
   each token contributes once.
3. **The durable final response is the single persisted message.** The live
   bubble is cleared on `stream_complete`/tombstone; the final response chunk
   (which survives reload) is rendered from `messages`. Transient deltas are
   never added to `messages` (only to `allChunks` for the raw viewer).
4. **Shared hiding.** `hiddenChunkIds` collects tombstone targets and
   `flow.signal = "delete"` targets; `chatMessagesFrom`, `rawViewChunks`, the
   chunk viewer list, and history loading all apply it. Raw mode filters
   per-token deltas and control chunks but keeps meaningful rows, and shows the
   model + backend badge.

**Consequences**:

- Streaming renders as one growing bubble, then is replaced by the durable
  response with no duplication, live or after reload.
- Correctness depends on chunk ids being unique and stable (they are).
- The renderer must know the annotation vocabulary (`chat.is_streaming`,
  `chat.model`, `chat.stream_complete`, `cafe.flow.tombstone`,
  `cafe.flow.signal`/`flow.target_chunk_id`); changes there touch the web.
- The live bubble carries no model annotation of its own, so in Raw mode its
  model/backend is stamped from the session's latest runtime config.

**Alternatives considered**:

- *Render tokens directly from `allChunks`.* Rejected: it is exactly what caused
  one bubble per token (and Raw mode).
- *Depend on the producer publishing a tombstone for every token.* Rejected:
  tombstones are best-effort; the client must dedupe and hide regardless.
- *Reconstruct the final text from deltas only.* Rejected: the durable
  `response_chunk` is the source of truth and the only thing in history.

Related: [ADR-135](./adr-135-chunk-deletion.md),
[ADR-005](./adr-005-transient-sole-persistence-gate.md),
[ADR-104](./adr-104-transient-chunk-retention.md).
