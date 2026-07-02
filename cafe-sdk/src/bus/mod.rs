mod reconnect;
mod wait;

pub use reconnect::run_with_reconnect;
pub use wait::wait_for_bus;

use crate::error::SdkError;
use cafe_types::{Chunk, ClientMessage, ServerMessage, SessionConfig, SessionInfo};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::warn;

/// A handle to the cafe-bus Unix socket.
///
/// Short-lived operations (publish, create_session, etc.) open a fresh
/// connection per call. Long-lived subscriptions spawn a background task
/// that forwards `ServerMessage` values over an `mpsc` channel.
#[derive(Clone)]
pub struct BusClient {
    socket_path: Arc<String>,
}

impl BusClient {
    /// Create a new bus client handle for the given socket path.
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: Arc::new(socket_path.into()),
        }
    }

    /// The socket path this client is configured to use.
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Open a fresh connection to the bus.
    async fn connect(
        &self,
    ) -> Result<
        (tokio::net::unix::OwnedWriteHalf, tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>),
        SdkError,
    > {
        let stream = UnixStream::connect(self.socket_path.as_str())
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;
        let (reader, writer) = stream.into_split();
        let lines = BufReader::new(reader).lines();
        Ok((writer, lines))
    }

    /// Write a single `ClientMessage` to the bus. Returns the write half.
    async fn send(
        &self,
        msg: &ClientMessage,
    ) -> Result<
        (tokio::net::unix::OwnedWriteHalf, tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>),
        SdkError,
    > {
        let (mut writer, lines) = self.connect().await?;
        let payload = serde_json::to_string(msg)? + "\n";
        writer.write_all(payload.as_bytes()).await?;
        Ok((writer, lines))
    }

    /// Publish a chunk to a session.
    pub async fn publish(&self, session_id: &str, chunk: Chunk) -> Result<(), SdkError> {
        let (_writer, _lines) = self
            .send(&ClientMessage::Publish {
                session_id: session_id.to_string(),
                chunk,
            })
            .await?;
        Ok(())
    }

    /// Create a new session.
    pub async fn create_session(
        &self,
        session_id: &str,
        agent_id: &str,
        config: SessionConfig,
    ) -> Result<(), SdkError> {
        let (_writer, _lines) = self
            .send(&ClientMessage::CreateSession {
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                config,
            })
            .await?;
        Ok(())
    }

    /// Delete a session.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SdkError> {
        let (_writer, _lines) = self
            .send(&ClientMessage::DeleteSession {
                session_id: session_id.to_string(),
            })
            .await?;
        Ok(())
    }

    /// List all sessions from the bus.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, SdkError> {
        let (_writer, mut lines) = self.send(&ClientMessage::ListSessions).await?;
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<ServerMessage>(&line) {
                Ok(ServerMessage::SessionsList { sessions }) => return Ok(sessions),
                Ok(ServerMessage::Error { message, code, .. }) => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(vec![])
    }

    /// Fetch the full history of a session by subscribing and draining
    /// until `HistoryComplete`. Returns an error if the session does not
    /// exist.
    pub async fn get_history(&self, session_id: &str) -> Result<Vec<Chunk>, SdkError> {
        let (_writer, mut lines) = self
            .send(&ClientMessage::Subscribe {
                session_id: session_id.to_string(),
            })
            .await?;

        let mut chunks = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<ServerMessage>(&line) {
                Ok(ServerMessage::Chunk { chunk, .. }) => chunks.push(chunk),
                Ok(ServerMessage::HistoryComplete { .. }) => break,
                Ok(ServerMessage::Error { message, code, .. }) => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(chunks)
    }

    /// Subscribe to a session and return a channel receiver of
    /// `ServerMessage` values. Spawns a background task that forwards
    /// messages until the connection drops or the receiver is dropped.
    pub async fn subscribe(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let (writer, mut lines) = self
            .send(&ClientMessage::Subscribe {
                session_id: session_id.to_string(),
            })
            .await?;

        let (tx, rx) = mpsc::channel::<ServerMessage>(256);

        tokio::spawn(async move {
            let _writer = writer;
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ServerMessage>(&line) {
                    Ok(msg) => {
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("cafe-sdk: invalid message from bus: {}", e);
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Subscribe to all sessions.
    pub async fn subscribe_all(&self) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let (writer, mut lines) = self.send(&ClientMessage::SubscribeAll).await?;

        let (tx, rx) = mpsc::channel::<ServerMessage>(1024);

        tokio::spawn(async move {
            let _writer = writer;
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ServerMessage>(&line) {
                    Ok(msg) => {
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("cafe-sdk: invalid message from bus: {}", e);
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Send a ping to the bus and wait for a pong.
    pub async fn ping(&self) -> Result<(), SdkError> {
        let (_writer, mut lines) = self.send(&ClientMessage::Ping).await?;
        while let Ok(Some(line)) = lines.next_line().await {
            if matches!(
                serde_json::from_str::<ServerMessage>(&line),
                Ok(ServerMessage::Pong)
            ) {
                return Ok(());
            }
        }
        Err(SdkError::Timeout)
    }
}
