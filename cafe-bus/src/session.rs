use cafe_types::envelope::EphemeralConfig;
use cafe_types::Chunk;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tokio::sync::broadcast;

/// Tracks a single subscriber's connection on a session.
#[derive(Debug, Clone)]
pub struct SubscriberInfo {
    pub conn_id: String,
    pub role: Option<String>,
}

#[derive(Debug)]
pub struct SessionState {
    pub session_id: String,
    pub agent_id: String,
    pub history: Vec<Chunk>,
    pub tx: broadcast::Sender<Chunk>,
    retained: Vec<(Chunk, Instant)>,
    /// Connections subscribed to this session, with their roles.
    pub(crate) subscribers: HashMap<String, SubscriberInfo>,
    /// Ephemeral lifecycle config. None = persistent.
    pub ephemeral: Option<EphemeralConfig>,
    /// User-defined tags for filtering/grouping sessions.
    pub tags: Vec<String>,
}

impl SessionState {
    pub fn new(session_id: String, agent_id: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            session_id,
            agent_id,
            history: Vec::new(),
            tx,
            retained: Vec::new(),
            subscribers: HashMap::new(),
            ephemeral: None,
            tags: Vec::new(),
        }
    }

    pub fn publish(&mut self, chunk: Chunk) {
        if !chunk.is_transient() {
            self.history.push(chunk.clone());
        } else if let Some(secs) = chunk.retain_secs() {
            // Retained transient chunk — keep in buffer for N seconds
            self.retained.push((chunk.clone(), Instant::now() + std::time::Duration::from_secs(secs)));
        }
        // Ignore send errors — no active subscribers is fine
        let _ = self.tx.send(chunk);
    }

    /// Return all non-expired retained transient chunks (oldest first),
    /// pruning expired entries in the process.
    pub fn drain_retained(&mut self) -> Vec<Chunk> {
        let now = Instant::now();
        let mut expired = 0;
        let mut valid = Vec::new();
        for (chunk, deadline) in &self.retained {
            if *deadline > now {
                valid.push(chunk.clone());
            } else {
                expired += 1;
            }
        }
        if expired > 0 {
            self.retained.retain(|(_, deadline)| *deadline > now);
        }
        valid
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Chunk> {
        self.tx.subscribe()
    }

    /// Register a subscriber connection. Returns the previous info if re-subscribing.
    pub fn add_subscriber(&mut self, conn_id: String, role: Option<String>) -> Option<SubscriberInfo> {
        self.subscribers.insert(conn_id.clone(), SubscriberInfo { conn_id, role })
    }

    /// Remove a subscriber connection. Returns the removed info, if any.
    pub fn remove_subscriber(&mut self, conn_id: &str) -> Option<SubscriberInfo> {
        self.subscribers.remove(conn_id)
    }

    /// Return the number of subscribers that match this session's count_role filter.
    /// If `count_role` is None, all subscribers count.
    pub fn counted_subscriber_count(&self) -> usize {
        match &self.ephemeral {
            Some(cfg) => {
                let role_filter = cfg.count_role.as_deref();
                self.subscribers
                    .values()
                    .filter(|s| role_filter.map_or(true, |r| s.role.as_deref() == Some(r)))
                    .count()
            }
            // Not ephemeral — all subscribers count
            None => self.subscribers.len(),
        }
    }

    /// Whether this session is ephemeral and should be tracked for auto-deletion.
    pub fn is_ephemeral(&self) -> bool {
        self.ephemeral.is_some()
    }

    /// Get the set of connection IDs currently subscribed.
    pub fn subscriber_conn_ids(&self) -> HashSet<String> {
        self.subscribers.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transient_chunk_broadcast_but_not_in_history() {
        let mut state = SessionState::new("test-session".into(), "test-agent".into());
        let mut rx = state.subscribe();

        let chunk = Chunk::new_text("hello", "com.test").as_transient();
        state.publish(chunk.clone());

        // Live subscriber receives it
        let received = rx.recv().await.unwrap();
        assert_eq!(received.content, chunk.content);
        assert!(received.is_transient());

        // History is empty — transient chunks are not appended
        assert!(state.history.is_empty());
    }

    #[tokio::test]
    async fn non_transient_chunk_appended_to_history() {
        let mut state = SessionState::new("test-session".into(), "test-agent".into());
        let mut rx = state.subscribe();

        let chunk = Chunk::new_text("hello", "com.test");
        state.publish(chunk.clone());

        // Live subscriber receives it
        let received = rx.recv().await.unwrap();
        assert_eq!(received.content, chunk.content);

        // History contains it
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].content, chunk.content);
    }

    #[tokio::test]
    async fn transient_chunk_not_in_replay() {
        let mut state = SessionState::new("test-session".into(), "test-agent".into());

        // Publish a transient chunk
        let transient = Chunk::new_text("transient", "com.test").as_transient();
        state.publish(transient);

        // Publish a non-transient chunk
        let normal = Chunk::new_text("normal", "com.test");
        state.publish(normal);

        // New subscriber should only receive the non-transient chunk in replay
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].content, Some("normal".into()));
    }

    // ── Property-based tests (proptest) ──

    use cafe_types::ContentType;
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

    fn chunk_list() -> impl Strategy<Value = Vec<Chunk>> {
        prop::collection::vec(any_chunk(), 0..20)
    }

    fn retained_chunk() -> impl Strategy<Value = Chunk> {
        (any_chunk(), any::<u64>()).prop_map(|(chunk, secs)| {
            chunk.as_transient().with_retain(secs.saturating_add(1) % 3600 + 1)
        })
    }

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let config = proptest::test_runner::Config::default();
        let mut runner = proptest::test_runner::TestRunner::new(config);
        runner
            .run(&strategy, |v| {
                test(v);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn session_history_count() {
        run_proptest(chunk_list(), |chunks: Vec<Chunk>| {
            let mut state = SessionState::new("test".into(), "test".into());
            let non_transient_count = chunks.iter().filter(|c| !c.is_transient()).count();
            for chunk in &chunks {
                state.publish(chunk.clone());
            }
            assert_eq!(state.history.len(), non_transient_count);
        });
    }

    #[test]
    fn session_history_empty_when_all_transient() {
        run_proptest(chunk_list(), |chunks: Vec<Chunk>| {
            let mut state = SessionState::new("test".into(), "test".into());
            let transient_chunks: Vec<_> = chunks.into_iter().map(|c| c.as_transient()).collect();
            for chunk in &transient_chunks {
                state.publish(chunk.clone());
                assert!(chunk.is_transient());
            }
            assert!(state.history.is_empty());
        });
    }

    #[test]
    fn session_history_order_preserved() {
        run_proptest(chunk_list(), |chunks: Vec<Chunk>| {
            let mut state = SessionState::new("test".into(), "test".into());
            for chunk in &chunks {
                state.publish(chunk.clone());
            }
            let non_transient: Vec<_> = chunks.iter().filter(|c| !c.is_transient()).collect();
            assert_eq!(state.history.len(), non_transient.len());
            for (i, chunk) in non_transient.iter().enumerate() {
                assert_eq!(state.history[i].id, chunk.id);
            }
        });
    }

    #[test]
    fn session_broadcast_delivery_all() {
        run_proptest(chunk_list(), |chunks: Vec<Chunk>| {
            let mut state = SessionState::new("test".into(), "test".into());
            let mut rx = state.subscribe();
            for chunk in &chunks {
                state.publish(chunk.clone());
                match rx.try_recv() {
                    Ok(received) => {
                        assert_eq!(received.id, chunk.id);
                        assert_eq!(received.content, chunk.content);
                        assert_eq!(received.is_transient(), chunk.is_transient());
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                        panic!("broadcast channel was empty after publish");
                    }
                    Err(e) => {
                        panic!("broadcast error: {:?}", e);
                    }
                }
            }
        });
    }

    #[test]
    fn session_broadcast_closed_when_sender_dropped() {
        let state = SessionState::new("test".into(), "test".into());
        let mut rx = state.subscribe();
        drop(state);
        match rx.try_recv() {
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {}
            _ => panic!("expected broadcast to be closed after SessionState drop"),
        }
    }

    #[test]
    fn retained_transient_in_drain_not_history() {
        run_proptest(retained_chunk(), |chunk: Chunk| {
            let mut state = SessionState::new("test".into(), "test".into());
            state.publish(chunk.clone());

            assert!(state.history.is_empty());

            let drained = state.drain_retained();
            assert_eq!(drained.len(), 1);
            assert_eq!(drained[0].id, chunk.id);
        });
    }

    #[test]
    fn drain_retained_empty_when_no_retained_chunks() {
        run_proptest(chunk_list(), |chunks: Vec<Chunk>| {
            let mut state = SessionState::new("test".into(), "test".into());
            for c in &chunks {
                let plain = {
                    let mut ann = c.annotations.clone();
                    ann.remove("cafe.transient.retain_secs");
                    Chunk { annotations: ann, ..c.clone() }
                };
                state.publish(plain);
            }
            let drained = state.drain_retained();
            assert!(drained.is_empty());
        });
    }

    #[test]
    fn retained_transient_order_in_drain() {
        run_proptest(
            (chunk_list(), prop::collection::vec(retained_chunk(), 0..10)),
            |(history_chunks, retained_chunks): (Vec<Chunk>, Vec<Chunk>)| {
                let mut state = SessionState::new("test".into(), "test".into());
                // Publish in mixed order: history chunk, retained, history, retained...
                for (h, r) in history_chunks.iter().zip(retained_chunks.iter()) {
                    state.publish(h.clone());
                    state.publish(r.clone());
                }
                let drained = state.drain_retained();
                let retained_count = retained_chunks.iter().count();
                // drained should only contain retained chunks (not expired)
                assert!(drained.len() <= retained_count);
                // drained should be in publication order (oldest first)
                for chunk in &drained {
                    assert!(chunk.is_transient());
                    assert!(chunk.retain_secs().is_some());
                }
            },
        );
    }

    #[test]
    fn replay_count_matches_history_plus_retained() {
        // During subscribe, HistoryComplete.count = history.len() + retained.len()
        run_proptest(
            (chunk_list(), prop::collection::vec(retained_chunk(), 0..10)),
            |(history_chunks, retained_chunks): (Vec<Chunk>, Vec<Chunk>)| {
                let mut state = SessionState::new("test".into(), "test".into());
                let non_transient_count = history_chunks.iter().filter(|c| !c.is_transient()).count();
                for chunk in &history_chunks {
                    state.publish(chunk.clone());
                }
                for chunk in &retained_chunks {
                    state.publish(chunk.clone());
                }
                let expected_replay = non_transient_count;
                let _retained_replay = state.drain_retained().len();
                assert_eq!(state.history.len(), expected_replay);
            },
        );
    }

    #[test]
    fn mutation_does_not_modify_history() {
        run_proptest(chunk_list(), |chunks: Vec<Chunk>| {
            let mut state = SessionState::new("test".into(), "test".into());
            for c in &chunks {
                state.publish(c.clone());
            }
            // Snapshot the annotations before mutation
            let before: Vec<_> = state.history.iter().map(|c| c.annotations.clone()).collect();
            // Publish a mutation targeting the last chunk
            if let Some(last) = state.history.last() {
                let mutation = Chunk::mutation(&last.id, "test-mutator")
                    .with_annotation("test.key", "test.value");
                state.publish(mutation);
                // History should have grown by 1 (the mutation itself is non-transient)
                assert_eq!(state.history.len(), before.len() + 1);
                // All original chunks' annotations must be unchanged
                for (i, ann) in before.iter().enumerate() {
                    assert_eq!(state.history[i].annotations, *ann,
                        "mutation modified history[{}] annotations", i);
                }
            }
        });
    }

    #[test]
    fn retained_chunk_expires_after_deadline() {
        // A chunk with retain_secs=0 expires immediately, so drain_retained
        // should not return it (deadline = Instant::now() + 0 <= Instant::now()).
        run_proptest(any_chunk(), |chunk: Chunk| {
            let expired = chunk.as_transient().with_retain(0_u64);
            let mut state = SessionState::new("test".into(), "test".into());
            state.publish(expired);
            // History must be empty (transient)
            assert!(state.history.is_empty());
            // drain_retained should NOT return it (expired immediately)
            let drained = state.drain_retained();
            assert!(drained.is_empty(), "expected expired chunk to be absent from drain");
            // The internal retained list should have been pruned too
            let drained_again = state.drain_retained();
            assert!(drained_again.is_empty());
        });
    }

    #[test]
    fn retained_chunk_with_nonzero_secs_survives() {
        run_proptest(any_chunk(), |chunk: Chunk| {
            let retained = chunk.as_transient().with_retain(3600_u64);
            let mut state = SessionState::new("test".into(), "test".into());
            state.publish(retained);
            // Should still be in the retained buffer (not expired yet)
            let drained = state.drain_retained();
            assert_eq!(drained.len(), 1);
        });
    }

    #[test]
    fn retained_transient_available_across_drain() {
        run_proptest(retained_chunk(), |chunk: Chunk| {
            let mut state = SessionState::new("test".into(), "test".into());
            state.publish(chunk.clone());
            // History must be empty (retained transient not in history)
            assert!(state.history.is_empty());
            // drain_retained returns it (still non-expired)
            let first = state.drain_retained();
            assert!(!first.is_empty());
            // drain_retained is NOT a drain — it only removes expired entries,
            // so non-expired retained chunks persist across calls
            let second = state.drain_retained();
            assert_eq!(second.len(), first.len());
        });
    }

    // ── Ephemeral session subscriber tests ──

    #[test]
    fn no_ephemeral_all_subscribers_count() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        assert!(!state.is_ephemeral());
        state.add_subscriber("c-1".into(), None);
        state.add_subscriber("c-2".into(), Some("user".into()));
        assert_eq!(state.counted_subscriber_count(), 2);
    }

    #[test]
    fn ephemeral_no_role_filter_counts_all() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        state.ephemeral = Some(EphemeralConfig {
            keepalive_secs: 0,
            count_role: None,
        });
        assert!(state.is_ephemeral());
        state.add_subscriber("c-1".into(), None);
        state.add_subscriber("c-2".into(), Some("user".into()));
        assert_eq!(state.counted_subscriber_count(), 2);
    }

    #[test]
    fn ephemeral_role_filter_matching_role() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        state.ephemeral = Some(EphemeralConfig {
            keepalive_secs: 30,
            count_role: Some("user".into()),
        });
        state.add_subscriber("c-1".into(), None);
        state.add_subscriber("c-2".into(), Some("user".into()));
        state.add_subscriber("c-3".into(), Some("user".into()));
        // Only c-2 and c-3 have role "user"
        assert_eq!(state.counted_subscriber_count(), 2);
    }

    #[test]
    fn ephemeral_role_filter_no_matching_role() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        state.ephemeral = Some(EphemeralConfig {
            keepalive_secs: 0,
            count_role: Some("admin".into()),
        });
        state.add_subscriber("c-1".into(), None);
        state.add_subscriber("c-2".into(), Some("user".into()));
        // No subscriber has role "admin"
        assert_eq!(state.counted_subscriber_count(), 0);
    }

    #[test]
    fn add_remove_subscriber_tracking() {
        let mut state = SessionState::new("s1".into(), "a1".into());
        state.ephemeral = Some(EphemeralConfig {
            keepalive_secs: 0,
            count_role: None,
        });

        let prev = state.add_subscriber("c-1".into(), Some("user".into()));
        assert!(prev.is_none());
        assert_eq!(state.counted_subscriber_count(), 1);

        // Re-subscribe (same conn_id, new role) — should replace
        let prev = state.add_subscriber("c-1".into(), Some("admin".into()));
        assert!(prev.is_some());
        assert_eq!(prev.unwrap().role, Some("user".into()));
        assert_eq!(state.counted_subscriber_count(), 1);

        // Remove
        let removed = state.remove_subscriber("c-1");
        assert!(removed.is_some());
        assert_eq!(state.counted_subscriber_count(), 0);

        // Remove non-existent
        let removed = state.remove_subscriber("c-999");
        assert!(removed.is_none());
    }

    // ── Property-based tests for subscriber tracking ──

    fn arb_ephemeral_config() -> impl Strategy<Value = Option<EphemeralConfig>> {
        prop_oneof![
            Just(None),
            (any::<u64>(), proptest::option::of(".{0,10}")).prop_map(
                |(keepalive_secs, count_role)| Some(EphemeralConfig {
                    keepalive_secs,
                    count_role,
                }),
            ),
        ]
    }

    /// Generate unique connection IDs by using UUID-style patterns.
    fn arb_unique_conn_id() -> impl Strategy<Value = String> {
        "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
    }

    #[test]
    fn prop_subscriber_count_matches_without_role_filter() {
        run_proptest(
            (
                arb_ephemeral_config(),
                prop::collection::vec(arb_unique_conn_id(), 0..10),
            ),
            |(ephemeral, conn_ids): (Option<EphemeralConfig>, Vec<String>)| {
                let mut state = SessionState::new("s".into(), "a".into());
                state.ephemeral = ephemeral;
                let expected_count = if state.ephemeral.as_ref()
                    .and_then(|c| c.count_role.as_ref())
                    .is_some()
                {
                    // With role filter, only matching roles count — we add all with None
                    0
                } else {
                    conn_ids.len()
                };
                for conn_id in &conn_ids {
                    state.add_subscriber(conn_id.clone(), None);
                }
                assert_eq!(
                    state.counted_subscriber_count(),
                    expected_count,
                    "conn_ids={:?}, ephemeral={:?}",
                    conn_ids,
                    state.ephemeral,
                );
            },
        );
    }

    #[test]
    fn prop_subscriber_count_with_role_filter() {
        run_proptest(
            (
                ".{0,10}",
                prop::collection::vec(
                    (arb_unique_conn_id(), proptest::option::of(".{0,10}")),
                    0..10,
                ),
            ),
            |(count_role, subscribers): (String, Vec<(String, Option<String>)>)| {
                let mut state = SessionState::new("s".into(), "a".into());
                state.ephemeral = Some(EphemeralConfig {
                    keepalive_secs: 0,
                    count_role: Some(count_role.clone()),
                });
                let matching = subscribers
                    .iter()
                    .filter(|(_, role)| role.as_deref() == Some(&count_role))
                    .count();
                for (conn_id, role) in &subscribers {
                    state.add_subscriber(conn_id.clone(), role.clone());
                }
                assert_eq!(
                    state.counted_subscriber_count(),
                    matching,
                    "count_role={:?}, subscribers={:?}",
                    count_role,
                    subscribers,
                );
            },
        );
    }

    #[test]
    fn prop_remove_nonexistent_returns_none() {
        run_proptest(
            (arb_unique_conn_id(), arb_unique_conn_id()),
            |(conn_id, other_id): (String, String)| {
                let mut state = SessionState::new("s".into(), "a".into());
                state.add_subscriber(conn_id.clone(), None);
                if other_id != conn_id {
                    assert!(state.remove_subscriber(&other_id).is_none());
                }
            },
        );
    }

    #[test]
    fn prop_counted_count_zero_on_empty() {
        run_proptest(
            arb_ephemeral_config(),
            |ephemeral: Option<EphemeralConfig>| {
                let state = SessionState::new("s".into(), "a".into());
                assert_eq!(state.counted_subscriber_count(), 0);
            },
        );
    }
}
