# ADR-100: SubscribeFiltered — bus-side chunk filtering

**Status**: Implemented (`abe6eea`)

**Context**: `SubscribeAll` forwards every chunk from every session. Services filter client-side, wasting bus bandwidth. cafe-store discards ~90% of chunks (transient filter). Binary-store would need only ~0.1% (binary-ref chunks). This doesn't scale to more services or higher throughput.

**Decision**: Add `SubscribeFiltered` — a new `ClientMessage` variant with a `SubscribeFilter` struct:

```rust
pub struct SubscribeFilter {
    pub sessions: Option<Vec<String>>,
    pub agents: Option<Vec<String>>,
    pub content_types: Option<Vec<ContentType>>,
    pub annotations: Option<HashMap<String, serde_json::Value>>,
}
```

The bus evaluates filters server-side at every forwarding point (history replay, retained chunks, live stream). Only matching chunks are sent to the subscriber. All specified dimensions must match (AND). Unspecified dimensions don't filter.

**Consequences**:
- cafe-store uses `SubscribeFiltered { annotations: Some({transient: false}) }` — RPC tokens never reach it
- Binary-store will use `SubscribeFiltered { content_types: Some([BinaryRef]) }`
- Reduced bus bandwidth for filtered subscribers
- `SubscribeAll` unchanged for backward compat
- Filter logic is pure: `chunk_matches_filter()` + `session_matches_filter()` — unit-testable
- Existing tests updated: 7 new tests for matcher functions
