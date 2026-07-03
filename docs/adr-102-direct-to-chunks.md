# ADR-102: Direct-to chunks

**Status**: Implemented (`2826eae`)

**Context**: Every chunk published to a session is broadcast to all subscribers. There's no way to send a chunk to a single connection — publishers can't privately reply to a specific subscriber without everyone else seeing it.

**Decision**: A `direct_to: connection_id` annotation on a `Publish` makes the bus skip broadcasting and route the chunk exclusively to that connection's writer. The bus looks up the target connection in `ConnectionRegistry` (ADR-101) and delivers the chunk directly. If the target connection doesn't exist, the bus returns `TARGET_NOT_FOUND` error.

Direct chunks are session-scoped (require a `session_id`) and always marked `transient` by the SDK's `publish_direct()` convenience method.

**Consequences**:
- Private messaging over the bus — only the intended recipient sees the chunk
- Binary-store can DM write credentials without exposing them to other subscribers
- Direct chunks are transient by convention (not persisted)
- Error handling for disconnected targets
- Foundation for request-response patterns over the bus
