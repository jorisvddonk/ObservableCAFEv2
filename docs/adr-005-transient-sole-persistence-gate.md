# ADR-005: Transient annotation as sole persistence gate

**Status**: Accepted

**Context**: Ephemeral chunks (streaming tokens, RPC messages, tombstone markers) should never be persisted or appear in history. Durable chunks (user messages, final responses) must be persisted. Without a unified rule, each service implements ad-hoc filtering, creating inconsistencies.

**Decision**: The `transient: true` annotation is the sole criterion for persistence. No exceptions:
- `transient: true` → broadcast to live subscribers only, never in history, never persisted
- `transient: false` or absent → in history, persisted by cafe-store

The bus enforces this in `SessionState::publish()`. Cafe-store checks `is_transient()` and skips. Every service follows the same rule.

**Consequences**:
- One check to rule them all — no special-case filters anywhere
- New chunk types automatically get correct persistence behavior
- Late subscribers miss transient chunks (led to ADR-104)
- RPC chunks are transient and thus not in history (required ADR-104's retention buffer)
