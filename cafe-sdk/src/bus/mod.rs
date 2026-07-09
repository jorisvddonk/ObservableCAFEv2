mod reconnect;
mod wait;

pub use reconnect::run_with_reconnect;
pub use wait::wait_for_bus;

use bytes::BytesMut;
use cafe_types::{BusCodec, BusCodecError, JsonLineCodec};
use cafe_types::{keys, Chunk, ClientMessage, ServerMessage, SessionConfig, SessionInfo};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::warn;

use crate::error::SdkError;

/// Framed reader that uses a `BusCodec` to extract messages from a byte stream.
pub struct BusReader<C: BusCodec, R: tokio::io::AsyncRead + Unpin> {
    reader: BufReader<R>,
    buf: BytesMut,
    _codec: std::marker::PhantomData<C>,
}

impl<C: BusCodec, R: tokio::io::AsyncRead + Unpin> BusReader<C, R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            buf: BytesMut::new(),
            _codec: std::marker::PhantomData,
        }
    }

    /// Read the next framed message, blocking until a complete frame arrives.
    pub async fn read_msg<M: serde::de::DeserializeOwned>(&mut self) -> Result<Option<M>, BusCodecError> {
        loop {
            if let Some((msg, consumed)) = C::decode(&self.buf)? {
                let _ = self.buf.split_to(consumed);
                return Ok(Some(msg));
            }
            let n = self.reader.read_buf(&mut self.buf).await?;
            if n == 0 {
                if let Some((msg, consumed)) = C::decode(&self.buf)? {
                    let _ = self.buf.split_to(consumed);
                    return Ok(Some(msg));
                }
                return Ok(None);
            }
        }
    }
}

/// A handle to the cafe-bus Unix socket.
///
/// Short-lived operations (publish, create_session, etc.) open a fresh
/// connection per call. Long-lived subscriptions spawn a background task
/// that forwards `ServerMessage` values over an `mpsc` channel.
#[derive(Clone)]
pub struct BusClient {
    socket_path: Arc<String>,
}

/// A session subscription with a persistent connection.
/// Publishing through this subscription uses the same bus connection,
/// so `source.connection` points to a live connection that can
/// receive `direct_to` replies (e.g. binary-store write credentials).
pub struct SessionSubscription<C: BusCodec = JsonLineCodec> {
    pub rx: mpsc::Receiver<ServerMessage>,
    writer: Option<tokio::net::unix::OwnedWriteHalf>,
    _reader_handle: tokio::task::JoinHandle<()>,
    session_id: String,
    role: Option<String>,
    _codec: std::marker::PhantomData<C>,
}

impl<C: BusCodec> SessionSubscription<C> {
    /// Publish a chunk on this subscription's connection.
    pub async fn publish(&mut self, chunk: Chunk) -> Result<(), SdkError> {
        let msg = ClientMessage::Publish {
            session_id: self.session_id.clone(),
            chunk,
        };
        let payload = C::encode(&msg)?;
        if let Some(ref mut writer) = self.writer {
            writer.write_all(&payload).await?;
        }
        Ok(())
    }
}

impl<C: BusCodec> Drop for SessionSubscription<C> {
    fn drop(&mut self) {
        self._reader_handle.abort();
        if let Some(writer) = self.writer.take() {
            drop(writer);
        }
    }
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

    /// Open a fresh connection, skip the initial Connected message.
    async fn connect<C: BusCodec>(
        &self,
    ) -> Result<
        (tokio::net::unix::OwnedWriteHalf, BusReader<C, tokio::net::unix::OwnedReadHalf>),
        SdkError,
    > {
        self.connect_with_role::<C>(None).await
    }

    /// Open a connection and optionally set connection metadata (role).
    async fn connect_with_role<C: BusCodec>(
        &self,
        role: Option<&str>,
    ) -> Result<
        (tokio::net::unix::OwnedWriteHalf, BusReader<C, tokio::net::unix::OwnedReadHalf>),
        SdkError,
    > {
        let stream = UnixStream::connect(self.socket_path.as_str())
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;
        let (reader, mut writer) = stream.into_split();
        let mut bus_reader = BusReader::<C, _>::new(reader);

        match bus_reader.read_msg::<ServerMessage>().await? {
            Some(ServerMessage::Connected { .. }) => {}
            Some(other) => warn!("expected Connected, got: {:?}", other),
            None => warn!("bus closed before sending Connected"),
        }

        // Optionally set connection metadata
        if let Some(r) = role {
            let meta_msg = ClientMessage::SetMeta { role: Some(r.to_string()) };
            let payload = C::encode(&meta_msg)?;
            use tokio::io::AsyncWriteExt;
            writer.write_all(&payload).await?;
        }

        Ok((writer, bus_reader))
    }

    /// Write a single `ClientMessage` to the bus. Returns the write half and reader.
    async fn send<C: BusCodec>(
        &self,
        msg: &ClientMessage,
    ) -> Result<
        (tokio::net::unix::OwnedWriteHalf, BusReader<C, tokio::net::unix::OwnedReadHalf>),
        SdkError,
    > {
        let (mut writer, reader) = self.connect::<C>().await?;
        let payload = C::encode(msg)?;
        writer.write_all(&payload).await?;
        Ok((writer, reader))
    }

    /// Publish a chunk to a session using the default JSON codec.
    pub async fn publish(&self, session_id: &str, chunk: Chunk) -> Result<(), SdkError> {
        self.publish_with_codec::<JsonLineCodec>(session_id, chunk).await
    }

    /// Publish a chunk with a specific codec.
    pub async fn publish_with_codec<C: BusCodec>(&self, session_id: &str, chunk: Chunk) -> Result<(), SdkError> {
        let (_writer, _reader) = self
            .send::<C>(&ClientMessage::Publish {
                session_id: session_id.to_string(),
                chunk,
            })
            .await?;
        Ok(())
    }

    /// Publish a chunk directly to a specific connection (private message over bus).
    pub async fn publish_direct(
        &self,
        target_connection: &str,
        session_id: &str,
        chunk: Chunk,
    ) -> Result<(), SdkError> {
        self.publish_direct_with_codec::<JsonLineCodec>(target_connection, session_id, chunk).await
    }

    /// Publish a chunk directly with a specific codec.
    pub async fn publish_direct_with_codec<C: BusCodec>(
        &self,
        target_connection: &str,
        session_id: &str,
        chunk: Chunk,
    ) -> Result<(), SdkError> {
        let chunk = chunk
            .with_annotation(keys::CAFE_DIRECT_TO, target_connection)
            .as_transient();
        let (_writer, _reader) = self
            .send::<C>(&ClientMessage::Publish {
                session_id: session_id.to_string(),
                chunk,
            })
            .await?;
        Ok(())
    }

    /// Create a new session using the default JSON codec.
    pub async fn create_session(
        &self,
        session_id: &str,
        agent_id: &str,
        config: SessionConfig,
    ) -> Result<(), SdkError> {
        self.create_session_with_codec::<JsonLineCodec>(session_id, agent_id, config).await
    }

    /// Create a new session with a specific codec.
    pub async fn create_session_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
        agent_id: &str,
        config: SessionConfig,
    ) -> Result<(), SdkError> {
        let (_writer, _reader) = self
            .send::<C>(&ClientMessage::CreateSession {
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                config,
            })
            .await?;
        Ok(())
    }

    /// Delete a session using the default JSON codec.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SdkError> {
        self.delete_session_with_codec::<JsonLineCodec>(session_id).await
    }

    /// Delete a session with a specific codec.
    pub async fn delete_session_with_codec<C: BusCodec>(&self, session_id: &str) -> Result<(), SdkError> {
        let (_writer, _reader) = self
            .send::<C>(&ClientMessage::DeleteSession {
                session_id: session_id.to_string(),
            })
            .await?;
        Ok(())
    }

    /// Set the tags for a session using the default JSON codec.
    pub async fn set_tags(&self, session_id: &str, tags: Vec<String>) -> Result<(), SdkError> {
        self.set_tags_with_codec::<JsonLineCodec>(session_id, tags).await
    }

    /// Set the tags for a session with a specific codec.
    pub async fn set_tags_with_codec<C: BusCodec>(&self, session_id: &str, tags: Vec<String>) -> Result<(), SdkError> {
        let (_writer, mut reader) = self
            .send::<C>(&ClientMessage::SetSessionTags {
                session_id: session_id.to_string(),
                tags,
            })
            .await?;
        // Wait for SessionTagsUpdated or Error response
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::SessionTagsUpdated { .. } => return Ok(()),
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// List all sessions from the bus using the default JSON codec.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, SdkError> {
        self.list_sessions_with_codec::<JsonLineCodec>().await
    }

    /// List all sessions with a specific codec.
    pub async fn list_sessions_with_codec<C: BusCodec>(&self) -> Result<Vec<SessionInfo>, SdkError> {
        let (_writer, mut reader) = self.send::<C>(&ClientMessage::ListSessions).await?;
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::SessionsList { sessions } => return Ok(sessions),
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(vec![])
    }

    /// Fetch the full history of a session using the default JSON codec.
    pub async fn get_history(&self, session_id: &str) -> Result<Vec<Chunk>, SdkError> {
        self.get_history_with_codec::<JsonLineCodec>(session_id).await
    }

    /// Fetch session history with a specific codec.
    pub async fn get_history_with_codec<C: BusCodec>(&self, session_id: &str) -> Result<Vec<Chunk>, SdkError> {
        let (_writer, mut reader) = self
            .send::<C>(&ClientMessage::Subscribe {
                session_id: session_id.to_string(),
            })
            .await?;

        let mut chunks = Vec::new();
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::Chunk { chunk, .. } => chunks.push(chunk),
                ServerMessage::HistoryComplete { .. } => break,
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(chunks)
    }

    /// Subscribe to a session using the default JSON codec.
    pub async fn subscribe_session(
        &self,
        session_id: &str,
    ) -> Result<SessionSubscription<JsonLineCodec>, SdkError> {
        self.subscribe_session_with_codec::<JsonLineCodec>(session_id).await
    }

    /// Subscribe to a session with a connection role (for ephemeral session lifecycle).
    /// The role is declared to the bus and used by ephemeral sessions to determine
    /// which subscribers count toward session lifetime.
    pub async fn subscribe_session_with_role(
        &self,
        session_id: &str,
        role: &str,
    ) -> Result<SessionSubscription<JsonLineCodec>, SdkError> {
        self.subscribe_session_with_codec_and_role::<JsonLineCodec>(session_id, Some(role))
            .await
    }

    /// Subscribe to a session with a specific codec.
    pub async fn subscribe_session_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
    ) -> Result<SessionSubscription<C>, SdkError> {
        self.subscribe_session_with_codec_and_role(session_id, None).await
    }

    /// Subscribe to a session with a specific codec and an optional connection role.
    async fn subscribe_session_with_codec_and_role<C: BusCodec>(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<SessionSubscription<C>, SdkError> {
        let (writer, mut reader) = {
            let (mut writer, reader) = self.connect_with_role::<C>(role).await?;
            let payload = C::encode(&ClientMessage::Subscribe {
                session_id: session_id.to_string(),
            })?;
            writer.write_all(&payload).await?;
            (writer, reader)
        };

        let (tx, rx) = mpsc::channel::<ServerMessage>(256);
        let sid = session_id.to_string();

        let reader_handle = tokio::spawn(async move {
            loop {
                match reader.read_msg::<ServerMessage>().await {
                    Ok(Some(msg)) => {
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("cafe-sdk: bus decode error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(SessionSubscription {
            rx,
            writer: Some(writer),
            _reader_handle: reader_handle,
            session_id: sid,
            role: role.map(String::from),
            _codec: std::marker::PhantomData,
        })
    }

    /// Subscribe to a session and return a channel receiver. (Default JSON codec.)
    pub async fn subscribe(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        self.subscribe_with_codec::<JsonLineCodec>(session_id).await
    }

    /// Subscribe to a session with a connection role (for ephemeral lifecycle).
    pub async fn subscribe_with_role(
        &self,
        session_id: &str,
        role: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        self.subscribe_with_codec_and_role::<JsonLineCodec>(session_id, Some(role)).await
    }

    /// Subscribe to a session with a specific codec.
    pub async fn subscribe_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        self.subscribe_with_codec_and_role::<C>(session_id, None).await
    }

    /// Subscribe with an optional connection role.
    async fn subscribe_with_codec_and_role<C: BusCodec>(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let (mut writer, mut reader) = self.connect_with_role::<C>(role).await?;

        let payload = C::encode(&ClientMessage::Subscribe {
            session_id: session_id.to_string(),
        })?;
        writer.write_all(&payload).await?;

        let (tx, rx) = mpsc::channel::<ServerMessage>(256);

        tokio::spawn(async move {
            let _writer = writer;
            loop {
                match reader.read_msg::<ServerMessage>().await {
                    Ok(Some(msg)) => {
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("cafe-sdk: bus decode error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Subscribe to all sessions matching a filter. (Default JSON codec.)
    pub async fn subscribe_filtered(
        &self,
        filter: cafe_types::SubscribeFilter,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        self.subscribe_filtered_with_codec::<JsonLineCodec>(filter).await
    }

    /// Subscribe to all sessions matching a filter with a specific codec.
    pub async fn subscribe_filtered_with_codec<C: BusCodec>(
        &self,
        filter: cafe_types::SubscribeFilter,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let (writer, mut reader) = self
            .send::<C>(&ClientMessage::SubscribeFiltered { filter })
            .await?;

        let (tx, rx) = mpsc::channel::<ServerMessage>(1024);

        tokio::spawn(async move {
            let _writer = writer;
            loop {
                match reader.read_msg::<ServerMessage>().await {
                    Ok(Some(msg)) => {
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("cafe-sdk: bus decode error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Subscribe to all sessions. (Default JSON codec.)
    pub async fn subscribe_all(&self) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        self.subscribe_all_with_codec::<JsonLineCodec>().await
    }

    /// Subscribe to all sessions with a specific codec.
    pub async fn subscribe_all_with_codec<C: BusCodec>(&self) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let (writer, mut reader) = self.send::<C>(&ClientMessage::SubscribeAll).await?;

        let (tx, rx) = mpsc::channel::<ServerMessage>(1024);

        tokio::spawn(async move {
            let _writer = writer;
            loop {
                match reader.read_msg::<ServerMessage>().await {
                    Ok(Some(msg)) => {
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("cafe-sdk: bus decode error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Send a ping to the bus and wait for a pong. (Default JSON codec.)
    pub async fn ping(&self) -> Result<(), SdkError> {
        self.ping_with_codec::<JsonLineCodec>().await
    }

    /// Ping with a specific codec.
    pub async fn ping_with_codec<C: BusCodec>(&self) -> Result<(), SdkError> {
        let (_writer, mut reader) = self.send::<C>(&ClientMessage::Ping).await?;
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            if matches!(msg, ServerMessage::Pong) {
                return Ok(());
            }
        }
        Err(SdkError::Timeout)
    }
}
