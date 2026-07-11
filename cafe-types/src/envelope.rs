use crate::chunk::{Chunk, ContentType};
use crate::session::SessionInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Ephemeral session lifecycle config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "bincode-codec", derive(bincode::Encode, bincode::Decode))]
pub struct EphemeralConfig {
    /// Seconds to keep session alive after the last counted subscriber disconnects.
    /// 0 = delete immediately.
    pub keepalive_secs: u64,
    /// If set, only count subscribers whose connection has this role.
    /// Subscribers without a matching role are ignored for lifecycle purposes.
    ///
    /// Example: count_role = Some("user") means the session lives as long as at
    /// least one connection with role "user" is subscribed. Internal services
    /// (pipelines, store, etc.) that don't declare a role are not counted.
    pub count_role: Option<String>,
}

/// Configuration passed when creating a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "bincode-codec", derive(bincode::Encode, bincode::Decode))]
pub struct SessionConfig {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// If set, the session is ephemeral and will be auto-deleted.
    pub ephemeral: Option<EphemeralConfig>,
    /// Optional initial tags for the session.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Filter for chunk subscriptions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "bincode-codec", derive(bincode::Encode, bincode::Decode))]
pub struct SubscribeFilter {
    /// Only forward chunks from sessions with these IDs. None = all sessions.
    pub sessions: Option<Vec<String>>,
    /// Only forward chunks from sessions with these agent_ids. None = all agents.
    pub agents: Option<Vec<String>>,
    /// Only forward chunks matching these content types. None = all types.
    pub content_types: Option<Vec<ContentType>>,
    /// Only forward chunks whose annotations contain ALL specified key/value pairs.
    #[cfg_attr(feature = "bincode-codec", bincode(with_serde))]
    pub annotations: Option<HashMap<String, serde_json::Value>>,
    /// Only forward from sessions that have at least one of these tags.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Exclude sessions that have any of these tags.
    #[serde(default)]
    pub tags_exclude: Option<Vec<String>>,
}

/// Messages sent from a client to the bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "bincode-codec", derive(bincode::Encode, bincode::Decode))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe {
        session_id: String,
    },
    SubscribeAll,
    SubscribeFiltered {
        filter: SubscribeFilter,
    },
    Publish {
        session_id: String,
        chunk: Chunk,
    },
    SetMeta {
        /// Optional role for this connection. Used by ephemeral sessions to
        /// filter which subscribers count toward session lifecycle.
        #[serde(default)]
        role: Option<String>,
        /// Preferred codecs for protocol negotiation, ordered by priority.
        /// e.g. ["bincode", "json"]. If absent, the bus treats this as a
        /// legacy connection and continues with JSON.
        #[serde(default)]
        codecs: Option<Vec<String>>,
    },
    CreateSession {
        session_id: String,
        agent_id: String,
        #[serde(default)]
        config: SessionConfig,
    },
    DeleteSession {
        session_id: String,
    },
    SetSessionTags {
        session_id: String,
        tags: Vec<String>,
    },
    ListSessions,
    Ping,
}

/// Messages sent from the bus to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "bincode-codec", derive(bincode::Encode, bincode::Decode))]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerMessage {
    Connected {
        connection_id: String,
    },
    CodecSet {
        codec: String,
        connection_id: String,
    },
    Chunk {
        session_id: String,
        chunk: Chunk,
    },
    SessionCreated {
        session_id: String,
        agent_id: String,
    },
    SessionDeleted {
        session_id: String,
    },
    SessionsList {
        sessions: Vec<SessionInfo>,
    },
    HistoryComplete {
        session_id: String,
        count: usize,
    },
    SessionTagsUpdated {
        session_id: String,
        tags: Vec<String>,
    },
    Error {
        session_id: Option<String>,
        message: String,
        code: String,
    },
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_subscribe_tag() {
        let msg = ClientMessage::Subscribe {
            session_id: "abc".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""op":"subscribe""#));
    }

    #[test]
    fn client_message_ping_tag() {
        let msg = ClientMessage::Ping;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""op":"ping""#));
    }

    #[test]
    fn server_message_pong_tag() {
        let msg = ServerMessage::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""event":"pong""#));
    }

    #[test]
    fn server_message_error_tag() {
        let msg = ServerMessage::Error {
            session_id: None,
            message: "oops".into(),
            code: "INVALID_MESSAGE".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""event":"error""#));
    }

    #[test]
    fn roundtrip_publish() {
        use crate::chunk::Chunk;
        let chunk = Chunk::new_text("hello", "com.test");
        let msg = ClientMessage::Publish {
            session_id: "s1".into(),
            chunk,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        match back {
            ClientMessage::Publish { session_id, chunk } => {
                assert_eq!(session_id, "s1");
                assert_eq!(chunk.content, Some("hello".into()));
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Property-based tests (proptest) ──

    use proptest::prelude::*;

    fn any_content_type() -> impl Strategy<Value = ContentType> {
        prop_oneof![
            Just(ContentType::Text),
            Just(ContentType::Binary),
            Just(ContentType::BinaryRef),
            Just(ContentType::Null),
        ]
    }

    fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn annotation_map() -> impl Strategy<Value = std::collections::HashMap<String, serde_json::Value>> {
        prop::collection::hash_map("[a-z._-]{1,15}", arb_json_value(), 0..5)
    }

    fn any_chunk() -> impl Strategy<Value = Chunk> {
        (
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            any_content_type(),
            proptest::option::of(".{0,50}"),
            proptest::option::of(prop::collection::vec(any::<u8>(), 0..50)),
            proptest::option::of("[a-z/._-]{0,30}"),
            "[a-zA-Z0-9._-]{1,30}",
            annotation_map(),
            any::<i64>(),
        )
            .prop_map(
                |(id, content_type, content, data, mime_type, producer, annotations, timestamp)| {
                    Chunk {
                        id,
                        content_type,
                        content,
                        data,
                        mime_type,
                        producer,
                        annotations,
                        timestamp,
                    }
                },
            )
    }

    fn any_session_config() -> impl Strategy<Value = SessionConfig> {
        (
            proptest::option::of(".{0,30}"),
            proptest::option::of(".{0,30}"),
            proptest::option::of(".{0,50}"),
            proptest::option::of(-10.0f32..10.0f32),
            proptest::option::of(any::<u32>()),
            proptest::option::of(any_ephemeral_config()),
            proptest::option::of(prop::collection::vec(".{1,10}", 0..5)),
        )
            .prop_map(
                |(backend, model, system_prompt, temperature, max_tokens, ephemeral, tags)| SessionConfig {
                    backend,
                    model,
                    system_prompt,
                    temperature,
                    max_tokens,
                    ephemeral,
                    tags,
                },
            )
    }

    fn any_subscribe_filter() -> impl Strategy<Value = SubscribeFilter> {
        (
            proptest::option::of(prop::collection::vec(".{0,20}", 0..5)),
            proptest::option::of(prop::collection::vec(".{0,20}", 0..5)),
            proptest::option::of(prop::collection::vec(any_content_type(), 0..4)),
            proptest::option::of(annotation_map()),
            proptest::option::of(prop::collection::vec(".{1,10}", 0..5)),
            proptest::option::of(prop::collection::vec(".{1,10}", 0..5)),
        )
            .prop_map(
                |(sessions, agents, content_types, annotations, tags, tags_exclude)| SubscribeFilter {
                    sessions,
                    agents,
                    content_types,
                    annotations,
                    tags,
                    tags_exclude,
                },
            )
    }

    fn any_session_info() -> impl Strategy<Value = SessionInfo> {
        (
            ".{0,20}",
            ".{0,20}",
            proptest::option::of(".{0,30}"),
            prop::collection::vec(".{1,10}", 0..5),
            any::<bool>(),
            ".{0,20}",
            any::<usize>(),
            any::<i64>(),
        )
            .prop_map(
                |(session_id, agent_id, display_name, tags, is_background, ui_mode, message_count, created_at)| {
                    SessionInfo {
                        session_id,
                        agent_id,
                        display_name,
                        tags,
                        is_background,
                        ui_mode,
                        message_count,
                        created_at,
                    }
                },
            )
    }

    fn any_ephemeral_config() -> impl Strategy<Value = EphemeralConfig> {
        (any::<u64>(), proptest::option::of(".{0,20}")).prop_map(
            |(keepalive_secs, count_role)| EphemeralConfig {
                keepalive_secs,
                count_role,
            },
        )
    }

    fn any_client_message() -> impl Strategy<Value = ClientMessage> {
        prop_oneof![
            ".{0,20}".prop_map(|session_id| ClientMessage::Subscribe { session_id }),
            Just(ClientMessage::SubscribeAll),
            any_subscribe_filter()
                .prop_map(|filter| ClientMessage::SubscribeFiltered { filter }),
            (".{0,20}", any_chunk())
                .prop_map(|(session_id, chunk)| ClientMessage::Publish { session_id, chunk }),
            proptest::option::of(".{0,20}")
                .prop_map(|role| ClientMessage::SetMeta { role, codecs: None }),
            (".{0,20}", ".{0,20}", any_session_config()).prop_map(
                |(session_id, agent_id, config)| ClientMessage::CreateSession {
                    session_id,
                    agent_id,
                    config,
                },
            ),
            ".{0,20}"
                .prop_map(|session_id| ClientMessage::DeleteSession { session_id }),
            (".{0,20}", prop::collection::vec(".{1,10}", 0..5)).prop_map(
                |(session_id, tags)| ClientMessage::SetSessionTags { session_id, tags },
            ),
            Just(ClientMessage::Ping),
        ]
    }

    fn any_server_message() -> impl Strategy<Value = ServerMessage> {
        prop_oneof![
            ".{0,20}"
                .prop_map(|connection_id| ServerMessage::Connected { connection_id }),
            (".{1,10}", ".{0,20}").prop_map(|(codec, connection_id)| {
                ServerMessage::CodecSet {
                    codec,
                    connection_id,
                }
            }),
            (".{0,20}", any_chunk())
                .prop_map(|(session_id, chunk)| ServerMessage::Chunk { session_id, chunk }),
            (".{0,20}", ".{0,20}").prop_map(
                |(session_id, agent_id)| ServerMessage::SessionCreated {
                    session_id,
                    agent_id,
                },
            ),
            ".{0,20}"
                .prop_map(|session_id| ServerMessage::SessionDeleted { session_id }),
            prop::collection::vec(any_session_info(), 0..5)
                .prop_map(|sessions| ServerMessage::SessionsList { sessions }),
            (".{0,20}", any::<usize>()).prop_map(
                |(session_id, count)| ServerMessage::HistoryComplete {
                    session_id,
                    count,
                },
            ),
            (".{0,20}", prop::collection::vec(".{1,10}", 0..5)).prop_map(
                |(session_id, tags)| ServerMessage::SessionTagsUpdated { session_id, tags },
            ),
            (proptest::option::of(".{0,20}"), ".{0,20}", ".{0,20}").prop_map(
                |(session_id, message, code)| ServerMessage::Error {
                    session_id,
                    message,
                    code,
                },
            ),
            Just(ServerMessage::Pong),
        ]
    }

    proptest! {
        #[test]
        fn client_message_serde_roundtrip(msg in any_client_message()) {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ClientMessage = serde_json::from_str(&json).unwrap();
            let orig_val = serde_json::to_value(&msg).unwrap();
            let back_val = serde_json::to_value(&back).unwrap();
            prop_assert_eq!(orig_val, back_val);
        }

        #[test]
        fn server_message_serde_roundtrip(msg in any_server_message()) {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            let orig_val = serde_json::to_value(&msg).unwrap();
            let back_val = serde_json::to_value(&back).unwrap();
            prop_assert_eq!(orig_val, back_val);
        }

        #[test]
        fn subscribe_filter_serde_roundtrip(filter in any_subscribe_filter()) {
            let json = serde_json::to_string(&filter).unwrap();
            let back: SubscribeFilter = serde_json::from_str(&json).unwrap();
            let orig_val = serde_json::to_value(&filter).unwrap();
            let back_val = serde_json::to_value(&back).unwrap();
            prop_assert_eq!(orig_val, back_val);
        }

        #[test]
        fn session_config_serde_roundtrip(config in any_session_config()) {
            let json = serde_json::to_string(&config).unwrap();
            let back: SessionConfig = serde_json::from_str(&json).unwrap();
            let orig_val = serde_json::to_value(&config).unwrap();
            let back_val = serde_json::to_value(&back).unwrap();
            prop_assert_eq!(orig_val, back_val);
        }

        #[test]
        fn client_message_serde_has_correct_tag(msg in any_client_message()) {
            let json = serde_json::to_string(&msg).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            let op = parsed.get("op").and_then(|v| v.as_str());
            prop_assert!(op.is_some(), "client message must have 'op' tag");
        }

        #[test]
        fn server_message_serde_has_correct_tag(msg in any_server_message()) {
            let json = serde_json::to_string(&msg).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            let event = parsed.get("event").and_then(|v| v.as_str());
            prop_assert!(event.is_some(), "server message must have 'event' tag");
        }

        // ── Invalid JSON rejection (ADR-001) ──

        #[test]
        fn invalid_json_rejected_as_client_message(garbage in "[ -~]{0,100}") {
            let result = serde_json::from_str::<ClientMessage>(&garbage);
            match result {
                Ok(msg) => {
                    // If it happened to parse, verify the tag is present
                    let val = serde_json::to_value(&msg).unwrap();
                    prop_assert!(val.get("op").is_some(), "parsed but no op tag");
                }
                Err(_) => {} // expected — graceful rejection
            }
        }

        #[test]
        fn invalid_json_rejected_as_server_message(garbage in "[ -~]{0,100}") {
            let result = serde_json::from_str::<ServerMessage>(&garbage);
            match result {
                Ok(msg) => {
                    let val = serde_json::to_value(&msg).unwrap();
                    prop_assert!(val.get("event").is_some(), "parsed but no event tag");
                }
                Err(_) => {} // expected — graceful rejection
            }
        }

        // ── Connected message shape (ADR-101) ──

        #[test]
        fn connected_message_has_event_and_connection_id(conn_id in ".{0,30}") {
            let msg = ServerMessage::Connected { connection_id: conn_id.clone() };
            let json = serde_json::to_value(&msg).unwrap();
            prop_assert_eq!(json["event"].as_str(), Some("connected"));
            prop_assert_eq!(json["connection_id"].as_str(), Some(conn_id.as_str()));
        }
    }
}
