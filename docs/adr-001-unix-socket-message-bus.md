# ADR-001: Unix socket message bus

**Status**: Accepted

**Context**: Services need to communicate. Options included HTTP REST, a message queue (NATS, RabbitMQ), gRPC, or a custom protocol over Unix sockets. REST adds HTTP overhead and polling latency. Message queues add operational complexity. gRPC requires schema coupling and stream management.

**Decision**: A custom message bus over a Unix domain socket (`/tmp/cafe-bus.sock`). Messages are newline-delimited JSON (`ClientMessage` → bus, `ServerMessage` ← bus). The bus is the single backbone — all services communicate through it, never directly with each other.

**Consequences**:
- No HTTP overhead, low latency (Unix socket)
- No message broker dependency (single binary)
- Simple wire format (JSON, human-debuggable with `nc`)
- Services are decoupled — bus is the only coupling point
- Limited to single-host deployment (Unix socket can't cross machines)
- Reconnection logic needed for all clients (bus is stateful)
