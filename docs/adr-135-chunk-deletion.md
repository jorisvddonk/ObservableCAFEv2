# ADR-135: Chunk deletion via `cafe.flow.signal = "delete"`

**Status**: Implemented

**Date**: 2026-10-01

**Driver**: The chunk viewer needed a delete affordance, and
`DELETE /api/sessions/:id/chunks/:chunk_id` already existed — it published a
`cafe.flow.signal = "delete"` control chunk plus a target id — but **nothing
consumed the signal**. The target stayed in bus history and in the store DB, so
deleted chunks stayed visible and reappeared on reload.

**Context**: The event-sourced chunk model ([ADR-002](./adr-002-event-sourced-chunk-model.md))
treats chunks as immutable and append-only; deletion is therefore not a
mutation of a chunk but a **retirement** of one. It must work for live
subscribers (immediate display update) and for late/reconnecting clients
(history must no longer contain it), and it must survive a bus restart (the
store must drop it too). The existing tombstone mechanism
([ADR-005](./adr-005-transient-sole-persistence-gate.md) context,
`cafe.flow.tombstone`) hides *transient* token chunks from display; deletion is
a different case: a durable, targeted removal.

**Decision**:

1. **A delete signal is a control chunk.** `DELETE …/chunks/:id` publishes a
   non-transient null chunk with `cafe.flow.signal = "delete"` and
   `flow.target_chunk_id = <id>` (new `keys::FLOW_TARGET_CHUNK_ID`).
2. **The bus applies the signal.** On `SessionState::publish`, a delete signal
   removes the target from both `history` and the retained-transient buffer
   before broadcasting. History is therefore clean for any client that connects
   afterwards, and the signal is still broadcast so live clients react.
   `SessionState::remove_chunk(id)` is exposed for this.
3. **The store applies the signal.** `cafe-store` matches the signal and calls
   `db.delete_chunk(session_id, target)`, so the chunk does not return when the
   bus is restored from the DB on restart.
4. **Clients hide the target defensively.** The delete signal is itself
   broadcast, so the web keeps a `hiddenChunkIds` set (tombstones + delete
   targets) and filters both chat and raw views; this also covers the window
   where the signal has arrived but the deleted chunk was already in the local
   list.

**Consequences**:

- Deletion is durable and consistent across live clients, history replay, and
  restart — but it is **not** a compaction of the append-only log: the delete
  signal itself is a new durable chunk, so history grows by one control chunk
  per deletion.
- `seq` numbers are not renumbered; a deleted chunk leaves a gap. Consumers must
  key on chunk id (they already do), never on position.
- Deleting an unknown id is a no-op (the endpoint returns 204 regardless), so a
  stale client does not error.
- Forking copies history at fork time; a chunk deleted before the fork is
  absent from the fork, and one deleted after the fork is unaffected in it.
- Deletion is unconditional (no undo); the signal carries no soft-delete state.

**Alternatives considered**:

- *Mutate/erase the chunk in place.* Rejected: breaks the immutability contract
  of ADR-002 and gives no audit trail.
- *A soft-delete annotation on the target with read-side filtering only.*
  Rejected: the target still replays in history, so every consumer must
  implement the filter, and a bug there resurrects deleted content.
- *Bus-only removal, no store change.* Rejected: the chunk returns after a bus
  restart when the store rehydrates history.
- *Store-only removal.* Rejected: live clients would keep showing it until
  reload.

Related: [ADR-002](./adr-002-event-sourced-chunk-model.md),
[ADR-005](./adr-005-transient-sole-persistence-gate.md),
[ADR-104](./adr-104-transient-chunk-retention.md).
