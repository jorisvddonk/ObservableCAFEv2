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
        self.history.push(chunk.clone());
        // Ignore send errors — no active subscribers is fine
        let _ = self.tx.send(chunk);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Chunk> {
        self.tx.subscribe()
    }
}
