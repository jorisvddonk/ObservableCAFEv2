# ADR-117: Session Tags

**Status:** Accepted

**Driver:** UIs need to group, filter, and hide/show sessions by category. The existing classifiers (`agent_id`, `is_background`) are too coarse and partially broken (`is_background` is hardcoded to `false` everywhere).

## Context

Sessions have no tagging or categorization mechanism. The only classifiers are:

- `agent_id` — which agent pipeline processes the session (immutable, set at creation)
- `is_background` — boolean flag (hardcoded `false` — the wiring is incomplete)
- `display_name` — user-facing name (stored as annotation chunk, never read back into `SessionInfo`)

None of these support the use case of "show me work sessions" or "hide archived sessions" across different UIs (web, TUI, Telegram).

The existing mutable config pattern (model, backend, system_prompt) uses annotation chunks with lazy resolution via `resolve_session_config()`. This works for agent-runtime config but doesn't support bus-level filtering — the bus would need to scan chunk history to filter sessions, which is impractical.

## Decision

We introduce session tags with a **dual-write** mechanism: tags are stored both as a native field on `SessionState` (for fast bus-level filtering) and as annotation chunks in session history (for audit trail and persistence).

### Data model

```rust
// SessionInfo — returned by list_sessions
pub struct SessionInfo {
    pub session_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub tags: Vec<String>,          // NEW
    pub is_background: bool,
    pub ui_mode: String,
    pub message_count: usize,
    pub created_at: i64,
}

// SessionConfig — passed at creation
pub struct SessionConfig {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub ephemeral: Option<EphemeralConfig>,
    pub tags: Option<Vec<String>>,  // NEW
}

// SubscribeFilter — for bus-level filtering
pub struct SubscribeFilter {
    pub sessions: Option<Vec<String>>,
    pub agents: Option<Vec<String>>,
    pub content_types: Option<Vec<ContentType>>,
    pub annotations: Option<HashMap<String, serde_json::Value>>,
    pub tags: Option<Vec<String>>,           // NEW — positive match
    pub tags_exclude: Option<Vec<String>>,   // NEW — negative match
}
```

### Bus protocol

New client message:
```rust
SetSessionTags { session_id: String, tags: Vec<String> }
```

New server event:
```rust
SessionTagsUpdated { session_id: String, tags: Vec<String> }
```

### Dual-write flow

When `SetSessionTags` is received:

1. Publish a null chunk with annotation `session.tags` = `["tag1", "tag2"]` to the session history
2. Update `SessionState.tags` in memory
3. Broadcast `SessionTagsUpdated` event

During `CreateSession`, the same dual-write happens for initial tags (if `config.tags` is set).

### Filtering semantics

| Filter field | Behavior |
|---|---|
| `tags: ["work"]` | Only sessions whose tags include "work" |
| `tags_exclude: ["archived"]` | Exclude sessions whose tags include "archived" |
| Both set | Session must match `tags` AND not match `tags_exclude` |

Tags filtering uses OR semantics within each field — a session matches `tags: ["work", "urgent"]` if it has either tag.

### Persistence

Tags are persisted in cafe-store's SQLite `sessions` table as a JSON array column:
```sql
ALTER TABLE sessions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
```

On bus restart, cafe-store passes tags via `SessionConfig.tags` during restore.

### Tag validation

Tags are free-form non-empty strings without whitespace. Validation at API entry points (HTTP, bus).

## Consequences

### Positive

- **Fast bus-level filtering**: Tags are on `SessionState`, so `session_matches_filter` checks them in O(1) per session — no history scan needed
- **Audit trail**: Every tag change is an append-only annotation chunk in session history
- **Works with existing persistence**: cafe-store handles both the column (for fast restore) and the chunks (for replay)
- **Backwards compatible**: `#[serde(default)]` on `tags` field means old clients/sessions get empty vec
- **Mutable**: Tags can be changed at any point in a session's lifetime, unlike `agent_id` or `ephemeral`
- **Replaces `is_background` use case**: Users can tag sessions as `background` or `archived` and filter accordingly without fixing the broken `is_background` field

### Tradeoffs

- **New bus message type**: Adds `SetSessionTags` to `ClientMessage` and `SessionTagsUpdated` to `ServerMessage`
- **Dual-write complexity**: Two representations must be kept in sync (annotation chunk + native field)
- **Multiple tag-changed chunks in history**: Each tag change creates a new annotation chunk; historians see the full edit history

### Risk

- **Tag explosion**: No global tag registry — users can create any number of tags. Mitigation: UI can suggest existing tags via autocomplete from list_sessions responses.
- **Race condition on concurrent tag updates**: Last write wins (tags are replaced, not merged). Acceptable for UI-driven use.

## Alternatives considered

**Tags only as annotation chunks** (mechanism B): Follows the `config.session.name` pattern. Rejected because bus-level filtering would require scanning chunk history on every `SubscribeFiltered` evaluation, which is O(n) per session.

**Tags only as native field**: Simpler but loses audit trail. Rejected because history is a core principle (ADR-002).

**Tags as a separate metadata store**: Over-engineered for key-value tags on sessions.

**Hierarchical tags / tag groups**: Adds complexity without clear use case yet. Tags are flat `Vec<String>`.
