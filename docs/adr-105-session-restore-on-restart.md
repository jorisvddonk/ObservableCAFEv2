# ADR-105: Session restore on bus restart

**Status**: Implemented (`921d56e`, `4c3a1d3`)

**Context**: Sessions are in-memory in cafe-bus. On crash or deploy, all sessions vanish. Cafe-store has them in SQLite but doesn't recreate them in the bus registry. Background agents recreate themselves (cafe-agent-runtime), but user sessions are lost until manual reconnection.

**Decision**: On reconnect, cafe-store compares its local DB sessions against the bus's `list_sessions()`. Any session in the DB but missing from the bus is recreated via `create_session()`, then its non-transient chunks are replayed via `publish()`. The SubscribeAll snapshot + a 500ms wait ensures background sessions recreated by other services are already present, so cafe-store only restores truly missing user sessions.

**Consequences**:
- User sessions survive bus restarts
- Background sessions are skipped (cafe-agent-runtime recreates them faster)
- Chunks are replayed in order (history replay semantics)
- Re-published chunks trigger new mutations (safe — transient RPCs not in history)
- 500ms wait is heuristic — could be missed in edge cases
