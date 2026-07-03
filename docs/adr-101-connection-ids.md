# ADR-101: Connection IDs

**Status**: Implemented (`66b6f41`)

**Context**: The bus routes chunks by session. There's no way to address a specific connection — publishers can't send a chunk to a specific subscriber without broadcasting it to everyone. This makes request-response patterns (like delivering write credentials to the producer of a BinaryRef chunk) require workarounds like polling or side channels.

**Decision**: Assign a unique ID to every bus connection via an atomic counter (`c-1`, `c-2`, ...). On connect, the bus sends `ServerMessage::Connected { connection_id }` as the first message. Every published chunk gets an auto-injected `source.connection` annotation so receivers know who to reply to.

A `ConnectionRegistry` (`Arc<RwLock<HashMap<String, Arc<Mutex<OwnedWriteHalf>>>>>`) tracks live connections for direct routing.

**Consequences**:
- Any receiver can reply privately to the publisher via `direct_to` (ADR-102)
- Binary-store can DM write credentials directly to the producer
- SDK silently skips the `Connected` message (backward compat)
- Connections are addressable — enables targeted messaging without broadcast
- Connection IDs are ephemeral (don't survive restart)
