# ADR-116: Ephemeral Sessions with Connection Roles

**Status:** Implemented (commit pending)

**Driver:** MCP bridge creates short-lived sessions for tool calls (RPC), but there was no automatic cleanup — sessions lived in memory until explicitly deleted. Internal subscribers (agent runtime, store, LLM evaluator, etc.) incorrectly kept ephemeral sessions alive.

## Context

Sessions in Cafe are purely manual-lifecycle: they live in `SessionRegistry` memory until an explicit `DeleteSession` call. This works for long-lived user sessions but is wasteful for temporary sessions like MCP RPC (`_cafe_mcp_{uuid}`), which must be manually cleaned up in a `try/finally` style block.

The system has about 30 internal subscriber connections across services (agent-runtime pipeline, LLM evaluator, store, dice, sheetbot, tts, stt, comfy, knowledgebase, web-fetch, binary-store, HTTP proxy). These subscribe to sessions via `SubscribeAll`/`SubscribeFiltered`/`Subscribe` and should not count as "users keeping a session alive" for cleanup purposes.

## Decision

We introduce two concepts:

### 1. Connection Roles

Connections can optionally declare a role after connecting:

```rust
ClientMessage::SetMeta {
    role: Option<String>,
}
```

Sent by a client once after receiving `Connected`. Stored per-connection in the bus. Default is `None`. Internal services (agent-runtime, store, etc.) never send this → role stays `None`.

### 2. Ephemeral Session Config

A new field on `SessionConfig`:

```rust
pub struct EphemeralConfig {
    pub keepalive_secs: u64,
    pub count_role: Option<String>,
}
```

- `keepalive_secs`: Seconds to keep session alive after the last *counted* subscriber disconnects. 0 = delete immediately.
- `count_role`: Only count subscribers whose connection role matches. `None` = count all subscribers (backwards compatible).

### 3. Subscriber Tracking

`SessionState` now tracks subscriber connections with their roles:

```rust
pub struct SessionState {
    // ...
    pub(crate) subscribers: HashMap<String, SubscriberInfo>,
    pub ephemeral: Option<EphemeralConfig>,
}
```

`SubscriberInfo` holds `conn_id` and `role`. `counted_subscriber_count()` returns the number of subscribers matching the session's role filter.

### 4. Timer-Based Cleanup

When a session's counted subscriber count drops to zero, `SessionRegistry::schedule_deletion` spawns a delayed task that re-checks the count before deleting. If a new subscriber arrives during the grace period, the timer merely wastes a sleep — deletion only happens if the count is still zero.

### Key flow (MCP RPC example)

1. MCP bridge opens connection C1, sends `SetMeta { role: "mcp-rpc" }`
2. Creates session with `ephemeral: { keepalive_secs: 0, count_role: "mcp-rpc" }`
3. Subscribes with role "mcp-rpc" via `subscribe_with_role`
4. Internal services also subscribe (role = None), but are ignored for lifecycle
5. MCP bridge disconnects → C1 removed → counted subscribers = 0 → session deleted immediately
6. No explicit `delete_session` call needed

## Consequences

### Positive

- **Automatic cleanup**: Ephemeral sessions self-destruct without explicit deletion
- **Role isolation**: Internal subscribers are transparently ignored
- **Backwards compatible**: Existing code needs no changes; internal services continue without SetMeta
- **MCP simplification**: Removes `try/finally` pattern from `rpc_dispatch`
- **Agent support**: Agent TOML files can opt in with `ephemeral_keepalive_secs` and `ephemeral_count_role`

### Tradeoffs

- **Timer tasks**: Grace-period timers are not cancellable — they always run to completion, check the count, and only then delete. Slight overhead for sessions that re-acquire subscribers during the grace period.
- **No explicit unsubscribe**: Currently only connection-drop triggers cleanup. An explicit unsubscribe is not a protocol message (could be added later).
- **Role allocation**: Applications must pick role strings consistently. Collisions between roles are intentional — same role across transport types (WS, SSE, MCP) all count toward lifecycle.

### Risk

- **Orphaned connections**: If a connection declares role "user" and subscribes to 100 ephemeral sessions, all 100 stay alive as long as that connection is open. This is correct behavior but could surprise operators.
- **Timer task accumulation**: Each deletion spawns a tokio task. For keepalive=0 (immediate), no timer is created. For keepalive>0, one sleeping task per session is negligible.

## Alternatives considered

**Track by connection ID instead of role**: More precise but requires the session creator to know its own connection ID, which doesn't align with the SDK's "open fresh connection per call" pattern.

**Track by session creator**: The creating connection is different from the subscribing connection, so this doesn't help.

**Cancelable timers with oneshot channels**: Added complexity for marginal benefit — the non-cancelable design is simpler and correct.
