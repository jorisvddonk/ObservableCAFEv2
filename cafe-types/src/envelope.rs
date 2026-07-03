use crate::chunk::{Chunk, ContentType};
use crate::session::SessionInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration passed when creating a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// Filter for chunk subscriptions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubscribeFilter {
    /// Only forward chunks from sessions with these IDs. None = all sessions.
    pub sessions: Option<Vec<String>>,
    /// Only forward chunks from sessions with these agent_ids. None = all agents.
    pub agents: Option<Vec<String>>,
    /// Only forward chunks matching these content types. None = all types.
    pub content_types: Option<Vec<ContentType>>,
    /// Only forward chunks whose annotations contain ALL specified key/value pairs.
    pub annotations: Option<HashMap<String, serde_json::Value>>,
}

/// Messages sent from a client to the bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    CreateSession {
        session_id: String,
        agent_id: String,
        #[serde(default)]
        config: SessionConfig,
    },
    DeleteSession {
        session_id: String,
    },
    ListSessions,
    Ping,
}

/// Messages sent from the bus to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerMessage {
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
}
