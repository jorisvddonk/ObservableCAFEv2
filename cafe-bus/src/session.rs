use cafe_types::Chunk;
use tokio::sync::broadcast;

pub struct SessionState {
    pub session_id: String,
    pub agent_id: String,
    pub history: Vec<Chunk>,
    pub tx: broadcast::Sender<Chunk>,
}

impl SessionState {
    pub fn new(session_id: String, agent_id: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            session_id,
            agent_id,
            history: Vec::new(),
            tx,
        }
    }

    pub fn publish(&mut self, chunk: Chunk) {
        // Transient chunks are broadcast to live subscribers but never
        // appended to history — they are ephemeral (e.g. streaming tokens).
        if !chunk.is_transient() {
            self.history.push(chunk.clone());
        }
        // Ignore send errors — no active subscribers is fine
        let _ = self.tx.send(chunk);
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
}
