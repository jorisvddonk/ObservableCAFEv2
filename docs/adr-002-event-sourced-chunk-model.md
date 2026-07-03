# ADR-002: Event-sourced chunk model

**Status**: Accepted

**Context**: The system needs to represent conversational state, LLM interactions, tool calls, file transfers, and configuration changes. A mutable state model (update-in-place database rows) would lose history, make debugging harder, and complicate replay for late-joining subscribers.

**Decision**: Every state change is an immutable `Chunk` appended to a per-session append-only log. Chunks are never modified or deleted (logical deletion via `flow.signal: "delete"` annotation). History replay replays the full log in order. New subscribers reconstruct state by replaying history.

**Consequences**:
- Full audit trail — every message, every config change is preserved
- Late subscribers can reconstruct any session state from history
- Simple bus protocol: append (Publish) + replay (Subscribe)
- Storage is append-only (no updates, no locking)
- Tombstones needed for ephemeral chunk removal (ADR-007)
- History grows unboundedly — cafe-store's DB can grow large
