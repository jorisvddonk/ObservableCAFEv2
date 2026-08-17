# ADR-123: Session forking

**Status**: Implemented

**Context**: Users need to branch a conversation off an existing session (e.g. continue a
thread in a different direction) without affecting the original. Today the only way to
create a session is a fresh, empty `CreateSession`. There is no way to start from an
existing session's state. The event-sourced model (ADR-002) keeps every chunk immutable
and append-only; a fork must reproduce a session's history without mutating the parent.

**Decision**: Add a native `ForkSession` bus operation that creates a new session whose
history is copied **verbatim** from a parent session, then becomes a live, independent
session.

- `ClientMessage::ForkSession { parent_session_id, session_id, config }` and
  `ServerMessage::SessionForked { parent_session_id, session_id }`.
- The fork copies the parent's **full history** (all non-transient chunks) **and** its
  non-expired **retained transient** chunks. No chunk is filtered.
- **No chunk field is modified on fork.** Content, annotations, and `timestamp` are
  preserved exactly. All chunks in a fork are semantically *"created in history"*, not at
  the moment of fork. This is required by ADR-124: chunk timestamps must be preserved
  across a fork.
- **No TTL adjustment on fork.** Retained transient chunks carry their original retention
  deadline into the fork unchanged, so a chunk's remaining TTL is preserved (a 5-day-TTL
  chunk forked at day 3 clears 2 days after the fork). The `transient.retain_secs`
  annotation is left at its original value.
- **Provenance**: an appended null chunk carrying a `fork.parent_id` annotation records
  the parent. `SessionInfo.parent_id` also surfaces the parent ID.

**Active-replay / RPC guard**: Bus tools (e.g. `llm`) **MUST NOT execute RPCs in a fork
prior to the forkpoint / `SessionCreated` emit.** This is enforced in two complementary
ways:

1. **Seed-then-insert**: The fork is seeded through a non-broadcasting path
   (`SessionState::seed`) and only made visible — via `insert()`, which emits
   `SessionCreated` — after seeding is complete. No chunk is broadcast and no
   `SessionCreated` is emitted before the fork is fully seeded, so no subscriber can
   observe the fork prior to its creation. `SessionForked` is sent to the requesting
   client as acknowledgement.

2. **HistoryComplete gating in RPC consumers**: Because a fork copies retained transient
   chunks (which can include actionable RPC requests), simply emitting `SessionCreated`
   would cause the fork's copied RPC chunks to be replayed to subscribers that dispatch on
   them. Every RPC-dispatching consumer (`llm`, `dice`, `knowledgebase`, `stt`, `rot13`,
   `comfy`, `sheetbot`, `hue`, `tts`, `epub`, `mcp-client`, `web-fetch`, `mcp-bridge`)
   therefore gates RPC/tool dispatch on the `HistoryComplete` marker: replayed history and
   retained chunks arrive before `HistoryComplete` and are ignored, while RPCs that arrive
   live after replay completes are dispatched. This matches the gate already used by
   `cafe-agent-runtime`'s `session_loop`.

**Consequences**:
- Forks are full, faithful snapshots of the parent's present state (history + live
  retained transients).
- The fork is automatically picked up by `cafe-llm` (via `subscribe_all` → `SessionCreated`)
  and persisted by `cafe-store` (via `SessionCreated` + chunk replay), same as ADR-105
  restore — no changes to those services.
- The parent session is never mutated.
- Forking does not re-fire RPCs and does not adjust TTL: nothing is broadcast during
  seeding, all chunk metadata is preserved, and RPC dispatch is gated on `HistoryComplete`.
- Chunks are deep-copied, so a fork's later appends never affect the parent.

**Alternatives considered**:
- *Context-only copy* (drop RPC/tool chunks) — rejected: not a faithful snapshot; relies
  on classifying chunk types, which is fragile.
- *Copy-by-reference* — rejected: `Vec<Chunk>` is immutable per session but forking needs
  a deep copy so the fork's future appends and the parent's are fully independent.
- *Client-side fork* (create + replay via `publish`) — rejected: broadcasting replayed
  chunks could trigger active replay; direct seeding avoids the broadcast path entirely.

Cross-references: [ADR-002](adr-002-event-sourced-chunk-model.md),
[ADR-005](adr-005-transient-sole-persistence-gate.md),
[ADR-105](adr-105-session-restore-on-restart.md), [ADR-124](adr-124-chunk-timestamps.md).
