use crate::session::SessionState;
use cafe_types::{ServerMessage, SessionInfo};
use std::collections::HashMap;
use tokio::sync::broadcast;

pub struct SessionRegistry {
    sessions: HashMap<String, SessionState>,
    event_tx: broadcast::Sender<ServerMessage>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            sessions: HashMap::new(),
            event_tx,
        }
    }

    pub fn event_tx(&self) -> broadcast::Sender<ServerMessage> {
        self.event_tx.clone()
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionState> {
        self.sessions.get(session_id)
    }

    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut SessionState> {
        self.sessions.get_mut(session_id)
    }

    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }

    pub fn insert(&mut self, state: SessionState) {
        let session_id = state.session_id.clone();
        let agent_id = state.agent_id.clone();
        self.sessions.insert(session_id.clone(), state);
        let _ = self.event_tx.send(ServerMessage::SessionCreated {
            session_id,
            agent_id,
        });
    }

    pub fn remove(&mut self, session_id: &str) -> bool {
        let removed = self.sessions.remove(session_id).is_some();
        if removed {
            let _ = self.event_tx.send(ServerMessage::SessionDeleted {
                session_id: session_id.to_string(),
            });
        }
        removed
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|s| SessionInfo {
                session_id: s.session_id.clone(),
                agent_id: s.agent_id.clone(),
                display_name: None,
                is_background: false,
                ui_mode: "chat".into(),
                message_count: s.history.len(),
                created_at: 0,
            })
            .collect()
    }
}
