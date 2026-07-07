use cafe_types::Chunk;
use std::time::Instant;
use tokio::sync::broadcast;

#[derive(Debug)]
pub struct SessionState {
    pub session_id: String,
    pub agent_id: String,
    pub history: Vec<Chunk>,
    pub tx: broadcast::Sender<Chunk>,
    retained: Vec<(Chunk, Instant)>,
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
}
