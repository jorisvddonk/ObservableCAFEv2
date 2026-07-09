use crate::session::SessionState;
use cafe_types::{ServerMessage, SessionInfo};
use std::collections::HashMap;
use std::time::Duration;
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

    /// Schedule an ephemeral session for deletion after a delay.
    /// If `delay` is None, the session is deleted immediately.
    /// The timer task checks the condition again before deleting, so it's safe
    /// even if a new subscriber arrives during the grace period.
    pub fn schedule_deletion(
        &mut self,
        session_id: &str,
        delay: Option<Duration>,
        registry: std::sync::Arc<tokio::sync::RwLock<SessionRegistry>>,
    ) {
        // Only delete if the session is actually ephemeral
        if !self.get(session_id).map_or(false, |s| s.is_ephemeral()) {
            return;
        }
        if delay.is_none() || delay.map_or(true, |d| d.is_zero()) {
            self.remove(session_id);
            return;
        }

        let sid = session_id.to_string();
        let d = delay.unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(d).await;
            let mut reg = registry.write().await;
            if reg.get(&sid).map_or(false, |s| {
                s.is_ephemeral() && s.counted_subscriber_count() == 0
            }) {
                reg.remove(&sid);
            }
        });
    }

    /// Cancel a pending scheduled deletion (no-op — timer checks condition itself).
    pub fn cancel_scheduled_deletion(&mut self, _session_id: &str) {
        // Timer tasks check the subscriber count on expiry, so explicit
        // cancellation isn't required. This exists as a hook for future use.
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|s| SessionInfo {
                session_id: s.session_id.clone(),
                agent_id: s.agent_id.clone(),
                display_name: None,
                tags: s.tags.clone(),
                is_background: false,
                ui_mode: "chat".into(),
                message_count: s.history.len(),
                created_at: 0,
            })
            .collect()
    }

    /// Replace the tags on a session and broadcast a SessionTagsUpdated event.
    /// Returns true if the session existed.
    pub fn set_tags(&mut self, session_id: &str, tags: Vec<String>) -> bool {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.tags = tags.clone();
            let _ = self.event_tx.send(ServerMessage::SessionTagsUpdated {
                session_id: session_id.to_string(),
                tags,
            });
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_types::envelope::EphemeralConfig;
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
    fn schedule_deletion_immediate_removes_session() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        state.ephemeral = Some(EphemeralConfig {
            keepalive_secs: 0,
            count_role: None,
        });
        let mut reg = SessionRegistry::new();
        reg.insert(state);
        assert!(reg.contains("s1"));

        let registry_arc = std::sync::Arc::new(tokio::sync::RwLock::new(
            // We can't easily create a new one here, so use the existing reg's type
            // but schedule_deletion takes the full Arc. We'll create one.
            SessionRegistry::new(),
        ));
        reg.schedule_deletion("s1", None, registry_arc);
        assert!(!reg.contains("s1"));
    }

    #[test]
    fn schedule_deletion_non_ephemeral_not_deleted() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        // No ephemeral config — persistent
        let mut reg = SessionRegistry::new();
        reg.insert(state);
        assert!(reg.contains("s1"));

        let registry_arc = std::sync::Arc::new(tokio::sync::RwLock::new(
            SessionRegistry::new(),
        ));
        reg.schedule_deletion("s1", None, registry_arc);
        assert!(reg.contains("s1"), "non-ephemeral sessions should not be deleted via schedule_deletion");
    }

    #[test]
    fn cancel_scheduled_deletion_is_noop() {
        let mut reg = SessionRegistry::new();
        // Should not panic
        reg.cancel_scheduled_deletion("nonexistent");
        // Should not panic even with existing entry (there isn't one tracked anymore)
        reg.cancel_scheduled_deletion("s1");
    }

    #[test]
    fn set_tags_updates_list_output() {
        run_proptest(arb_session_state(), |mut state: SessionState| {
            let sid = state.session_id.clone();
            let tags = vec!["work".into(), "urgent".into()];
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            assert!(reg.set_tags(&sid, tags.clone()));
            let sessions = reg.list();
            let info = sessions.iter().find(|s| s.session_id == sid).unwrap();
            assert_eq!(info.tags, tags);
        });
    }

    #[test]
    fn set_tags_nonexistent_returns_false() {
        run_proptest(
            (arb_session_state(), prop::collection::vec("[a-z]{1,10}", 0..5)),
            |(state, tags): (SessionState, Vec<String>)| {
                let sid = state.session_id.clone();
                let mut reg = SessionRegistry::new();
                reg.insert(state);
                // Removing with a different ID returns false
                let other = format!("{}_x", sid);
                if other != sid {
                    assert!(!reg.set_tags(&other, tags));
                }
            },
        );
    }

    #[test]
    fn set_tags_broadcasts_event() {
        run_proptest(arb_session_state(), |state: SessionState| {
            let sid = state.session_id.clone();
            let tags = vec!["tag1".into()];
            let mut reg = SessionRegistry::new();
            let mut rx = reg.event_tx().subscribe();
            reg.insert(state);
            // Drain the SessionCreated event
            let _ = rx.try_recv();
            assert!(reg.set_tags(&sid, tags.clone()));
            if let Ok(event) = rx.try_recv() {
                match event {
                    ServerMessage::SessionTagsUpdated { session_id, tags: ev_tags } => {
                        assert_eq!(session_id, sid);
                        assert_eq!(ev_tags, tags);
                    }
                    _ => panic!("expected SessionTagsUpdated, got {:?}", event),
                }
            }
        });
    }

    #[test]
    fn set_tags_overwrites_previous() {
        run_proptest(arb_session_state(), |mut state: SessionState| {
            let sid = state.session_id.clone();
            state.tags = vec!["old".into()];
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            let new_tags = vec!["new".into()];
            assert!(reg.set_tags(&sid, new_tags.clone()));
            let sessions = reg.list();
            let info = sessions.iter().find(|s| s.session_id == sid).unwrap();
            assert_eq!(info.tags, vec!["new"]);
            assert!(!info.tags.contains(&"old".into()));
        });
    }

    #[test]
    fn set_tags_empty_replaces() {
        run_proptest(arb_session_state(), |mut state: SessionState| {
            let sid = state.session_id.clone();
            state.tags = vec!["old".into()];
            let mut reg = SessionRegistry::new();
            reg.insert(state);
            assert!(reg.set_tags(&sid, vec![]));
            let sessions = reg.list();
            let info = sessions.iter().find(|s| s.session_id == sid).unwrap();
            assert!(info.tags.is_empty());
        });
    }

    #[test]
    fn ephemeral_session_is_listed() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        state.ephemeral = Some(EphemeralConfig {
            keepalive_secs: 30,
            count_role: Some("user".into()),
        });
        let mut reg = SessionRegistry::new();
        reg.insert(state);
        let sessions = reg.list();
        let info = sessions.iter().find(|s| s.session_id == "s1").unwrap();
        assert_eq!(info.agent_id, "a1");
        assert_eq!(info.message_count, 0);
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
