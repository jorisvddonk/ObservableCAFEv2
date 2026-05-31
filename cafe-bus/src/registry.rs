use crate::session::SessionState;
use cafe_types::SessionInfo;
use std::collections::HashMap;

pub struct SessionRegistry {
    sessions: HashMap<String, SessionState>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
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
        self.sessions.insert(state.session_id.clone(), state);
    }

    pub fn remove(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
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
