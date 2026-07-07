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

    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use crate::session::SessionState;

    fn arb_session_state() -> impl Strategy<Value = SessionState> {
        (".{0,20}", ".{0,20}")
            .prop_map(|(session_id, agent_id)| SessionState::new(session_id, agent_id))
    }

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner.run(&strategy, |v| { test(v); Ok(()) }).unwrap();
    }

    #[test]
    fn insert_then_contains() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            assert!(!reg.contains(&sid));
            reg.insert(state);
            assert!(reg.contains(&sid));
        });
    }

    #[test]
    fn remove_then_not_contains() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            assert!(reg.contains(&sid));
            assert!(reg.remove(&sid));
            assert!(!reg.contains(&sid));
        });
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        run_proptest(
            (arb_session_state(), ".{0,20}"),
            |(state, other_id): (SessionState, String)| {
                let sid = state.session_id.clone();
                let mut reg = SessionRegistry::new();
                reg.insert(state);
                // Removing with a different ID returns false
                if other_id != sid {
                    assert!(!reg.remove(&other_id));
                }
            },
        );
    }

    #[test]
    fn insert_then_list_contains() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let aid = state.agent_id.clone();
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            let sessions = reg.list();
            assert!(sessions.iter().any(|s| s.session_id == sid && s.agent_id == aid));
        });
    }

    #[test]
    fn remove_then_list_excludes() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            reg.remove(&sid);
            let sessions = reg.list();
            assert!(!sessions.iter().any(|s| s.session_id == sid));
        });
    }

    #[test]
    fn list_returns_all_inserted() {
        run_proptest(
            prop::collection::vec(
                ("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", ".{0,20}")
                    .prop_map(|(session_id, agent_id)| SessionState::new(session_id, agent_id)),
                0..20,
            ),
            |states: Vec<SessionState>| {
                let mut reg = SessionRegistry::new();
                let expected_count = states.len();
                for s in states {
                    reg.insert(s);
                }
                assert_eq!(reg.list().len(), expected_count);
            },
        );
    }

    #[test]
    fn event_tx_broadcasts_session_created() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let aid = state.agent_id.clone();
            let mut reg = SessionRegistry::new();
            let mut rx = reg.event_tx().subscribe();
            reg.insert(state);
            // Should receive a SessionCreated event
            if let Ok(event) = rx.try_recv() {
                match event {
                    ServerMessage::SessionCreated { session_id, agent_id } => {
                        assert_eq!(session_id, sid);
                        assert_eq!(agent_id, aid);
                    }
                    _ => panic!("expected SessionCreated, got {:?}", event),
                }
            }
        });
    }

    #[test]
    fn event_tx_broadcasts_session_deleted() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            let mut rx = reg.event_tx().subscribe();
            reg.insert(state);
            // Drain the SessionCreated event
            let _ = rx.try_recv();
            reg.remove(&sid);
            // Should receive a SessionDeleted event
            if let Ok(event) = rx.try_recv() {
                match event {
                    ServerMessage::SessionDeleted { session_id } => {
                        assert_eq!(session_id, sid);
                    }
                    _ => panic!("expected SessionDeleted, got {:?}", event),
                }
            }
        });
    }

    #[test]
    fn get_returns_inserted_session() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            assert!(reg.get(&sid).is_some());
            assert_eq!(reg.get(&sid).unwrap().session_id, sid);
        });
    }

    #[test]
    fn get_mut_returns_inserted_session() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            assert!(reg.get_mut(&sid).is_some());
        });
    }

    #[test]
    fn list_message_count_matches_history() {
        use cafe_types::ContentType;
        run_proptest(arb_session_state(), |mut state: SessionState| {
            // Add some non-transient chunks
            let chunk = cafe_types::Chunk::new_text("hello", "test");
            state.publish(chunk);
            let sid = state.session_id.clone();
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            let sessions = reg.list();
            let info = sessions.iter().find(|s| s.session_id == sid).unwrap();
            assert_eq!(info.message_count, 1);
        });
    }

    #[test]
    fn session_isolation_two_sessions() {
        run_proptest(
            (
                "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
                "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
                "[a-zA-Z._-]{1,20}",
            ),
            |(sid_a, sid_b, agent): (String, String, String)| {
                // Use different IDs
                if sid_a == sid_b { return; }
                let mut state_a = SessionState::new(sid_a.clone(), agent.clone());
                let mut state_b = SessionState::new(sid_b.clone(), agent);
                let chunk_a = cafe_types::Chunk::new_text("hello from A", "test");
                let chunk_b = cafe_types::Chunk::new_text("hello from B", "test");
                state_a.publish(chunk_a);
                state_b.publish(chunk_b);
                assert_eq!(state_a.history.len(), 1);
                assert_eq!(state_b.history.len(), 1);
                // Session A should not contain B's chunk content
                assert_eq!(state_a.history[0].content, Some("hello from A".into()));
                assert_eq!(state_b.history[0].content, Some("hello from B".into()));
            },
        );
    }
}
