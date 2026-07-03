# ADR-104: Transient chunk retention

**Status**: Implemented (`61a4921`)

**Context**: Transient chunks (ADR-005) are broadcast to live subscribers but never stored in history. If a subscriber joins after a transient chunk is published (e.g., after the pipeline dispatches an RPC but before cafe-llm subscribes), the chunk is lost forever. For RPC messages, this causes pipeline timeouts.

**Decision**: Transient chunks can opt into an in-memory retention buffer via `transient.retain_secs` annotation. The bus keeps a copy in a per-session `Vec<(Chunk, Instant)>`. New subscribers get non-expired retained chunks served after history replay but before `HistoryComplete`, preserving chronological ordering.

RPC chunks (`jsonrpc.request`, `jsonrpc.response`) carry `with_retain(60)` for a 60-second window. Streaming start signals carry `with_retain(5)`.

**Consequences**:
- Late subscribers receive transient chunks within the retention window
- 60-second window covers worst-case pipeline RPC timing
- Retained chunks are in-memory only — not persisted (follows ADR-005)
- Pruning is lazy (on `drain_retained()` calls during subscribe)
- Memory proportional to in-flight RPC volume (low — a few KB per session)
