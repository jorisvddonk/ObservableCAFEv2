# cafe-types — Build Guide

**Role:** Shared Rust library. Defines all data structures used across the system.
No runtime, no I/O, no async. Pure data + serialization.

**Build this first.** Every other Rust crate depends on it.

---

## Cargo.toml dependencies to add

```toml
[dependencies]
serde       = { workspace = true }
serde_json  = { workspace = true }
uuid        = { version = "1", features = ["v4", "serde"] }
thiserror   = "1"
base64      = "0.22"
```

---

## File structure

```
cafe-types/src/
```

---

## chunk.rs — what to implement

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Text,
    Binary,
    BinaryRef,
    Null,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,                              // UUID v4
    pub content_type: ContentType,
    pub content: Option<String>,                 // set iff content_type == Text
    #[serde(with = "base64_option")]
    pub data: Option<Vec<u8>>,                   // set iff content_type == Binary
    pub mime_type: Option<String>,
    pub producer: String,                        // reverse-DNS e.g. com.nominal.cafe-llm
    pub annotations: HashMap<String, serde_json::Value>,
    pub timestamp: i64,                          // Unix milliseconds
}

impl Chunk {
    pub fn new_text(content: impl Into<String>, producer: impl Into<String>) -> Self { ... }
    pub fn new_binary(data: Vec<u8>, mime_type: impl Into<String>, producer: impl Into<String>) -> Self { ... }
    pub fn new_binary_ref(mime_type: impl Into<String>, producer: impl Into<String>) -> Self { ... }
    pub fn new_null(producer: impl Into<String>) -> Self { ... }
    pub fn is_binary_ref(&self) -> bool { ... }

    /// Returns a clone of self with an additional annotation.
    pub fn with_annotation(self, key: impl Into<String>, value: impl Serialize) -> Self { ... }

    /// Convenience: get annotation as a specific type
    pub fn get_annotation<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> { ... }

    /// Convenience: get chat role
    pub fn role(&self) -> Option<&str> {
        self.annotations.get("chat.role")?.as_str()
    }

    /// True if this is a runtime config null chunk
    pub fn is_runtime_config(&self) -> bool {
        self.content_type == ContentType::Null
            && self.annotations.get("config.type").and_then(|v| v.as_str()) == Some("runtime")
    }
}
```

Implement a `base64_option` serde module for `Option<Vec<u8>>`:
- Serialize: `None` → `null`, `Some(bytes)` → base64 string
- Deserialize: `null` → `None`, string → base64-decode → `Some(bytes)`

---

## annotation.rs — what to implement

Define constants for all standard annotation keys from `docs/spec-cafe.md`:

```rust
pub mod keys {
    pub const CHAT_ROLE: &str = "chat.role";
    pub const CHAT_MODEL: &str = "chat.model";
    pub const CHAT_FINISH_REASON: &str = "chat.finish_reason";
    pub const CHAT_IS_STREAMING: &str = "chat.is_streaming";
    pub const CHAT_STREAM_COMPLETE: &str = "chat.stream_complete";
    pub const SESSION_NAME: &str = "session.name";
    pub const SECURITY_TRUST_LEVEL: &str = "security.trust-level";
    pub const CONFIG_TYPE: &str = "config.type";
    pub const CONFIG_BACKEND: &str = "config.backend";
    pub const CONFIG_MODEL: &str = "config.model";
    pub const CONFIG_SYSTEM_PROMPT: &str = "config.system_prompt";
    pub const WEB_SOURCE_URL: &str = "web.source_url";
    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";
    pub const FLOW_SIGNAL: &str = "flow.signal";

    // Binary asset keys
    pub const BINARY_WRITE_URL: &str = "cafe.binary.write_url";
    pub const BINARY_WRITE_TOKEN: &str = "cafe.binary.write_token";
    pub const BINARY_READ_URL: &str = "cafe.binary.read_url";
    pub const BINARY_READ_TOKEN: &str = "cafe.binary.read_token";
    pub const BINARY_BYTE_SIZE: &str = "cafe.binary.byte_size";
    pub const BINARY_COMPLETED: &str = "cafe.binary.completed";

    // Bus routing keys
    pub const SOURCE_CONNECTION: &str = "cafe.source.connection";
    pub const DIRECT_TO: &str = "cafe.direct_to";
    pub const MUTATES_TARGET_ID: &str = "cafe.mutates.target_id";

    // Transient keys
    pub const TRANSIENT: &str = "cafe.transient";
    pub const TRANSIENT_RETAIN_SECS: &str = "cafe.transient.retain_secs";
}

pub mod roles {
    pub const USER: &str = "user";
    pub const ASSISTANT: &str = "assistant";
    pub const SYSTEM: &str = "system";
}
```

---

## envelope.rs — what to implement

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe { session_id: String },
    SubscribeAll,
    SubscribeFiltered { session_id: String, filter: SubscribeFilter },
    Publish { session_id: String, chunk: Chunk },
    CreateSession { session_id: String, agent_id: String, config: SessionConfig },
    DeleteSession { session_id: String },
    ListSessions,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerMessage {
    Connected { connection_id: String },
    Chunk { session_id: String, chunk: Chunk },
    SessionCreated { session_id: String, agent_id: String },
    SessionDeleted { session_id: String },
    SessionsList { sessions: Vec<SessionInfo> },
    HistoryComplete { session_id: String, count: usize },
    Error { session_id: Option<String>, message: String, code: String },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeFilter {
    pub content_types: Option<Vec<ContentType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}
```

---

## Testing requirements

Write unit tests in each file:
- `Chunk` roundtrips through JSON without data loss
- Binary chunks correctly base64-encode/decode `data`
- `with_annotation` does not mutate the original
- `ClientMessage` and `ServerMessage` serialize with the correct `op`/`event` tag

Run with `cargo test -p cafe-types`.
