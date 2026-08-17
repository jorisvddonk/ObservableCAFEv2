# ADR-124: Chunk timestamps are authoritative and preserved on fork

**Status**: Accepted

**Context**: Every `Chunk` already carries a `timestamp: i64` (epoch milliseconds) set at
creation (`cafe-types/src/chunk.rs`). However, several derived behaviors rely on
process-local `Instant` deadlines (the retained-transient buffer in `cafe-bus`) rather than
on the chunk's own timestamp. This makes timing semantics inconsistent and makes it unclear
whether operations like forking (ADR-123) are allowed to touch a chunk's timestamp.

**Decision**:

1. **The `Chunk.timestamp` is authoritative.** It records when the chunk was created, in
   history. Derived behaviors that need a deadline should derive it from the chunk's
   timestamp plus its configured TTL, not from a separate process-local clock that gets
   re-armed.

2. **Timestamps MUST NOT be modified on fork.** A chunk copied into a forked session
   (ADR-123) is semantically *"created in history"* — its `timestamp` stays the same as in
   the parent. Forking does not re-stamp or re-age any chunk.

3. **Consequently, forking does not adjust TTL.** Because a chunk's TTL is derived from its
   (unchanged) creation timestamp, copying a transient chunk with a 5-day TTL at day 3
   leaves it with 2 days of remaining lifetime in the fork — the deadline is implied by the
   preserved timestamp, not recomputed at fork time. This is why ADR-123 preserves the
   retained deadline verbatim rather than re-arming it.

4. **Future work**: migrate the retained-transient buffer to compute deadlines from
   `Chunk.timestamp + TTL` uniformly, eliminating the separate `Instant` field, so all
   timing behavior is consistent and timestamp-derived.

**Consequences**:
- A fork is a faithful, timestamp-preserving snapshot; chunk provenance and age are
  identical between parent and fork.
- No operation may rewrite a chunk's `timestamp`; doing so would break the audit trail
  and the TTL semantics.
- The retained buffer's `Instant` deadline becomes a cache of `timestamp + TTL`, to be
  derived (rather than independently re-armed) per this ADR's forward note.

**Alternatives considered**:
- *Re-arm TTL on fork* (reset the deadline to a full TTL from fork time) — rejected: it
  would falsify a chunk's age and contradict the "created in history" semantics.
- *Strip or regenerate timestamps on fork* — rejected: breaks provenance and audit.

Cross-references: [ADR-002](adr-002-event-sourced-chunk-model.md),
[ADR-104](adr-104-transient-chunk-retention.md), [ADR-123](adr-123-session-forking.md).
