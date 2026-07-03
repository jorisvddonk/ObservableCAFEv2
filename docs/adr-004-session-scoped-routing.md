# ADR-004: Session-scoped routing

**Status**: Accepted

**Context**: Chunks need to be routed to interested subscribers. Global broadcast (everyone sees everything) doesn't scale. Per-topic routing would need a topic registry and subscriber management.

**Decision**: Every chunk belongs to exactly one **session**. Subscribers subscribe to sessions (or all sessions via `SubscribeAll`). A session is created explicitly (`CreateSession`), has a unique ID, and an `agent_id` that determines which pipeline processes it. Sessions are the unit of isolation, history, and routing.

**Consequences**:
- Routing is simple: subscribe to a session → get its chunks
- History is per-session — replay is bounded and scoped
- Sessions are the natural unit for UI (a chat conversation, a background task)
- Session lifecycle is explicit — must be created before publishing
- Bus restart loses all sessions (ADR-105)
