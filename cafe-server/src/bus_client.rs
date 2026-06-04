use anyhow::Result;
use cafe_types::{Chunk, ClientMessage, ServerMessage, SessionConfig, SessionInfo};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::warn;

/// A handle to the bus. Each operation opens a fresh connection so that
/// SSE streams (which need their own subscriber) don't share state.
#[derive(Clone)]
pub struct BusClient {
    socket_path: Arc<String>,
}

impl BusClient {
    pub fn new(socket_path: String) -> Self {
        Self {
            socket_path: Arc::new(socket_path),
        }
    }

    /// Open a fresh connection and send a single request, collecting responses
    /// until the connection closes or a predicate is satisfied.
    async fn open(&self) -> Result<(tokio::net::unix::OwnedWriteHalf, tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>)> {
        let stream = UnixStream::connect(self.socket_path.as_str()).await?;
        let (reader, writer) = stream.into_split();
        let lines = BufReader::new(reader).lines();
        Ok((writer, lines))
    }

    pub async fn publish(&self, session_id: &str, chunk: Chunk) -> Result<()> {
        let (mut writer, _lines) = self.open().await?;
        let msg = serde_json::to_string(&ClientMessage::Publish {
            session_id: session_id.to_string(),
            chunk,
        })? + "\n";
        writer.write_all(msg.as_bytes()).await?;
        Ok(())
    }

    pub async fn create_session(
        &self,
        session_id: &str,
        agent_id: &str,
        config: SessionConfig,
    ) -> Result<()> {
        let (mut writer, _lines) = self.open().await?;
        let msg = serde_json::to_string(&ClientMessage::CreateSession {
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            config,
        })? + "\n";
        writer.write_all(msg.as_bytes()).await?;
        Ok(())
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let (mut writer, _lines) = self.open().await?;
        let msg = serde_json::to_string(&ClientMessage::DeleteSession {
            session_id: session_id.to_string(),
        })? + "\n";
        writer.write_all(msg.as_bytes()).await?;
        Ok(())
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let (mut writer, mut lines) = self.open().await?;
        let msg = serde_json::to_string(&ClientMessage::ListSessions)? + "\n";
        writer.write_all(msg.as_bytes()).await?;

        // Read until we get a sessions_list response
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(ServerMessage::SessionsList { sessions }) = serde_json::from_str(&line) {
                return Ok(sessions);
            }
        }
        Ok(vec![])
    }

    /// Fetch the history of a session as a Vec<Chunk> by subscribing and
    /// draining until HistoryComplete. Returns an error if the session is
    /// not found.
    pub async fn get_history(&self, session_id: &str) -> Result<Vec<Chunk>> {
        let (mut writer, mut lines) = self.open().await?;
        let msg = serde_json::to_string(&ClientMessage::Subscribe {
            session_id: session_id.to_string(),
        })? + "\n";
        writer.write_all(msg.as_bytes()).await?;

        let mut chunks = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<ServerMessage>(&line) {
                Ok(ServerMessage::Chunk { chunk, .. }) => chunks.push(chunk),
                Ok(ServerMessage::HistoryComplete { .. }) => break,
                Ok(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message));
                }
                _ => {}
            }
        }
        Ok(chunks)
    }

    /// Subscribe to a session and return a channel receiver of chunks.
    /// Spawns a background task that forwards chunks until the connection closes.
    pub async fn subscribe(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>> {
        let (mut writer, mut lines) = self.open().await?;
        let msg = serde_json::to_string(&ClientMessage::Subscribe {
            session_id: session_id.to_string(),
        })? + "\n";
        writer.write_all(msg.as_bytes()).await?;

        let (tx, rx) = mpsc::channel::<ServerMessage>(256);

        tokio::spawn(async move {
            // Keep writer alive so the connection stays open
            let _writer = writer;
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ServerMessage>(&line) {
                    Ok(msg) => {
                        if tx.send(msg).await.is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(e) => {
                        warn!("bus_client: invalid message: {}", e);
                    }
                }
            }
        });

        Ok(rx)
    }
}
