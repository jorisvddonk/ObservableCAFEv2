# ADR-106: SubscribeAll migration

**Status**: Implemented (`08930f0`)

**Context**: Historically, cafe-agent-runtime, cafe-comfy, cafe-tts, and cafe-sheetbot used 2s polling (`list_sessions()` + `subscribe()`) to discover sessions. This created a 0-2 second window where transient RPC messages (dispatched by the pipeline) could be broadcast before the target service subscribed. With SubscribeAll (ADR-002) and transient retention (ADR-104) in place, polling was the remaining source of timing races.

**Decision**: Switch all four services from polling to `SubscribeAll`. Each service's `poll_sessions()` function is replaced with `subscribe_sessions()` that listens for `SessionCreated` events. The `HashSet`-based known-session tracking is removed entirely.

**Consequences**:
- Zero-delay session discovery across all services
- No more polling anywhere in the system
- Retained transient buffer (ADR-104) catches any remaining micro-races
- ~70 lines of polling boilerplate removed
- Bus bandwidth slightly increases (all services now see all chunks — mitigated by ADR-100)
