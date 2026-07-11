mod any_transport;
mod reconnect;
mod transport;
mod wait;

#[cfg(feature = "iroh-client")]
mod iroh_transport;

pub use any_transport::{AnyReader, AnyTransport, AnyWriter};
pub use reconnect::run_with_reconnect;
pub use transport::{BusTransport, UnixSocketTransport};
pub use wait::wait_for_bus;

#[cfg(feature = "iroh-client")]
pub use iroh_transport::{IrohConfig, IrohTransport};

use bytes::BytesMut;
use cafe_types::{BusCodec, BusCodecError, JsonLineCodec};
#[cfg(feature = "bincode-client")]
use cafe_types::BincodeLengthPrefixCodec;
use cafe_types::{keys, Chunk, ClientMessage, ServerMessage, SessionConfig, SessionInfo};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use tracing::warn;

use crate::error::SdkError;

/// Preferred codec for bus protocol negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCodec {
    /// NDJSON (newline-delimited JSON) — always supported.
    Json,
    /// Bincode with 4-byte LE length prefix — requires `bincode-client` feature.
    #[cfg(feature = "bincode-client")]
    Bincode,
}

impl ClientCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            #[cfg(feature = "bincode-client")]
            Self::Bincode => "bincode",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "json" => Some(Self::Json),
            #[cfg(feature = "bincode-client")]
            "bincode" => Some(Self::Bincode),
            _ => None,
        }
    }
}

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
    pub async fn read_msg<M: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<Option<M>, BusCodecError> {
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

    /// Consume the reader and return the inner transport reader, discarding
    /// any buffered data. Used after codec negotiation to switch codecs.
    pub fn into_inner(self) -> R {
        self.reader.into_inner()
    }
}

/// A handle to the cafe-bus.
///
/// Short-lived operations (publish, create_session, etc.) open a fresh
/// connection per call. Long-lived subscriptions spawn a background task
/// that forwards `ServerMessage` values over an `mpsc` channel.
///
/// The default transport is [`AnyTransport`], which supports both Unix
/// sockets and (with `iroh-client` feature) iroh QUIC at runtime.
#[derive(Clone)]
pub struct BusClient<T: BusTransport = AnyTransport> {
    transport: Arc<T>,
    preferred_codecs: Vec<ClientCodec>,
    negotiated_codec: Arc<Mutex<Option<ClientCodec>>>,
}

/// A session subscription with a persistent connection.
/// Publishing through this subscription uses the same bus connection,
/// so `source.connection` points to a live connection that can
/// receive `direct_to` replies (e.g. binary-store write credentials).
pub struct SessionSubscription<W: tokio::io::AsyncWrite + Unpin + Send + 'static = AnyWriter> {
    pub rx: mpsc::Receiver<ServerMessage>,
    writer: Option<W>,
    _reader_handle: tokio::task::JoinHandle<()>,
    session_id: String,
    role: Option<String>,
    codec: ClientCodec,
}

impl<W: tokio::io::AsyncWrite + Unpin + Send> SessionSubscription<W> {
    /// Publish a chunk on this subscription's connection.
    pub async fn publish(&mut self, chunk: Chunk) -> Result<(), SdkError> {
        let msg = ClientMessage::Publish {
            session_id: self.session_id.clone(),
            chunk,
        };
        let payload = match self.codec {
            ClientCodec::Json => JsonLineCodec::encode(&msg)?,
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => BincodeLengthPrefixCodec::encode(&msg)?,
        };
        if let Some(ref mut writer) = self.writer {
            writer.write_all(&payload).await?;
        }
        Ok(())
    }
}

impl<W: tokio::io::AsyncWrite + Unpin + Send> Drop for SessionSubscription<W> {
    fn drop(&mut self) {
        self._reader_handle.abort();
    }
}

impl BusClient {
    pub fn unix(socket_path: impl Into<String>) -> BusClient<AnyTransport> {
        BusClient {
            transport: Arc::new(AnyTransport::unix(socket_path)),
            preferred_codecs: vec![ClientCodec::Json],
            negotiated_codec: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_preferred_codecs(
        socket_path: impl Into<String>,
        codecs: Vec<ClientCodec>,
    ) -> BusClient<AnyTransport> {
        BusClient {
            transport: Arc::new(AnyTransport::unix(socket_path)),
            preferred_codecs: codecs,
            negotiated_codec: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(feature = "iroh-client")]
    pub fn any(transport: AnyTransport) -> BusClient<AnyTransport> {
        BusClient {
            transport: Arc::new(transport),
            preferred_codecs: vec![ClientCodec::Json],
            negotiated_codec: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(feature = "iroh-client")]
    pub async fn from_iroh_config(config: IrohConfig) -> Result<BusClient<AnyTransport>, SdkError> {
        let transport = config.bind().await?;
        Ok(Self::any(AnyTransport::Iroh(transport)))
    }
}

impl<T: BusTransport> BusClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport: Arc::new(transport),
            preferred_codecs: vec![ClientCodec::Json],
            negotiated_codec: Arc::new(Mutex::new(None)),
        }
    }

    pub fn new_with_codecs(transport: T, codecs: Vec<ClientCodec>) -> Self {
        Self {
            transport: Arc::new(transport),
            preferred_codecs: codecs,
            negotiated_codec: Arc::new(Mutex::new(None)),
        }
    }

    /// Negotiate the codec with the bus. Called lazily on first use.
    /// Opens a dedicated connection, sends `SetMeta` with `codecs` field,
    /// and reads the bus response (`CodecSet` or legacy `Connected`).
    async fn negotiate(&self) -> Result<ClientCodec, SdkError> {
        // Return cached value if already negotiated
        if let Some(codec) = *self.negotiated_codec.lock().await {
            return Ok(codec);
        }

        let codec = self.do_negotiate().await?;

        let mut guard = self.negotiated_codec.lock().await;
        if guard.is_none() {
            *guard = Some(codec);
        }
        Ok(codec)
    }

    async fn do_negotiate(&self) -> Result<ClientCodec, SdkError> {
        use tokio::io::AsyncWriteExt;

        let (mut writer, reader) = self.transport.connect().await?;

        // Send SetMeta as JSON with codecs
        let codec_strs: Vec<String> = self
            .preferred_codecs
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();
        let meta_msg = ClientMessage::SetMeta {
            role: None,
            codecs: Some(codec_strs),
        };
        let payload = JsonLineCodec::encode(&meta_msg)?;
        writer.write_all(&payload).await?;

        // Read response as JSON
        let mut json_reader = BusReader::<JsonLineCodec, _>::new(reader);
        match json_reader.read_msg::<ServerMessage>().await? {
            Some(ServerMessage::CodecSet { codec, .. }) => {
                writer.shutdown().await?;
                Ok(ClientCodec::from_str(&codec).unwrap_or(ClientCodec::Json))
            }
            Some(ServerMessage::Connected { .. }) => {
                writer.shutdown().await?;
                Ok(ClientCodec::Json)
            }
            Some(other) => {
                warn!("negotiate: unexpected response: {:?}", other);
                writer.shutdown().await?;
                Ok(ClientCodec::Json)
            }
            None => {
                warn!("negotiate: bus closed before responding");
                Ok(ClientCodec::Json)
            }
        }
    }

    /// Open a fresh connection, skip the initial Connected message.
    async fn connect<C: BusCodec>(
        &self,
    ) -> Result<(T::Writer, BusReader<C, T::Reader>), SdkError> {
        self.connect_with_role::<C>(None).await
    }

    /// Open a connection and optionally set connection metadata (role).
    /// Always sends the initial `SetMeta` as JSON and reads the bus response
    /// as JSON, regardless of the requested codec `C`. After the handshake
    /// completes, the connection switches to codec `C` for all subsequent messages.
    async fn connect_with_role<C: BusCodec>(
        &self,
        role: Option<&str>,
    ) -> Result<(T::Writer, BusReader<C, T::Reader>), SdkError> {
        let (mut writer, reader) = self.transport.connect().await?;

        let codec_strs: Vec<String> = self
            .preferred_codecs
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();
        let meta_msg = ClientMessage::SetMeta {
            role: role.map(|r| r.to_string()),
            codecs: if codec_strs.is_empty() { None } else { Some(codec_strs) },
        };
        let payload = JsonLineCodec::encode(&meta_msg)?;
        writer.write_all(&payload).await?;

        let mut json_reader = BusReader::<JsonLineCodec, _>::new(reader);
        match json_reader.read_msg::<ServerMessage>().await? {
            Some(ServerMessage::CodecSet { codec, .. }) => {
                if codec != C::NAME {
                    warn!(
                        "negotiated codec {} does not match requested {}",
                        codec,
                        C::NAME
                    );
                    return Err(SdkError::BusError {
                        message: format!(
                            "codec mismatch: bus chose {} but requested {}",
                            codec,
                            C::NAME
                        ),
                        code: Some("CODEC_MISMATCH".into()),
                    });
                }
            }
            Some(ServerMessage::Connected { .. }) => {
                // Legacy bus — always JSON. If we expected bincode, this
                // is a mismatch (should have been caught by negotiate()).
                let _ = role;
            }
            Some(other) => warn!("expected Connected/CodecSet, got: {:?}", other),
            None => warn!("bus closed before responding"),
        }

        let inner_reader = json_reader.into_inner();
        let bus_reader = BusReader::<C, _>::new(inner_reader);

        Ok((writer, bus_reader))
    }

    /// Write a single `ClientMessage` to the bus. Returns the write half and reader.
    async fn send<C: BusCodec>(
        &self,
        msg: &ClientMessage,
    ) -> Result<(T::Writer, BusReader<C, T::Reader>), SdkError> {
        let (mut writer, reader) = self.connect::<C>().await?;
        let payload = C::encode(msg)?;
        writer.write_all(&payload).await?;
        Ok((writer, reader))
    }

    /// Publish a chunk to a session.
    pub async fn publish(&self, session_id: &str, chunk: Chunk) -> Result<(), SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.publish_with_codec::<JsonLineCodec>(session_id, chunk).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.publish_with_codec::<BincodeLengthPrefixCodec>(session_id, chunk).await
            }
        }
    }

    /// Publish a chunk with a specific codec.
    pub async fn publish_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
        chunk: Chunk,
    ) -> Result<(), SdkError> {
        let (mut writer, _reader) = self
            .send::<C>(&ClientMessage::Publish {
                session_id: session_id.to_string(),
                chunk,
            })
            .await?;
        // Ensure the write is flushed before the stream is dropped.
        // QUIC streams are lazy — the peer won't see the message until
        // the write is acknowledged or the stream is shut down.
        writer.shutdown().await?;
        Ok(())
    }

    /// Publish a chunk directly to a specific connection (private message over bus).
    pub async fn publish_direct(
        &self,
        target_connection: &str,
        session_id: &str,
        chunk: Chunk,
    ) -> Result<(), SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.publish_direct_with_codec::<JsonLineCodec>(target_connection, session_id, chunk).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.publish_direct_with_codec::<BincodeLengthPrefixCodec>(target_connection, session_id, chunk).await
            }
        }
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
        let (mut writer, _reader) = self
            .send::<C>(&ClientMessage::Publish {
                session_id: session_id.to_string(),
                chunk,
            })
            .await?;
        writer.shutdown().await?;
        Ok(())
    }

    /// Create a new session.
    pub async fn create_session(
        &self,
        session_id: &str,
        agent_id: &str,
        config: SessionConfig,
    ) -> Result<(), SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.create_session_with_codec::<JsonLineCodec>(session_id, agent_id, config).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.create_session_with_codec::<BincodeLengthPrefixCodec>(session_id, agent_id, config).await
            }
        }
    }

    /// Create a new session with a specific codec.
    pub async fn create_session_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
        agent_id: &str,
        config: SessionConfig,
    ) -> Result<(), SdkError> {
        let (_writer, mut reader) = self
            .send::<C>(&ClientMessage::CreateSession {
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                config,
            })
            .await?;
        // Wait for SessionCreated or Error — keeps the stream alive
        // until the bus confirms. Critical for iroh where dropping the
        // stream early can abort the connection before processing.
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::SessionCreated { .. } => return Ok(()),
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Delete a session.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.delete_session_with_codec::<JsonLineCodec>(session_id).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.delete_session_with_codec::<BincodeLengthPrefixCodec>(session_id).await
            }
        }
    }

    /// Delete a session with a specific codec.
    pub async fn delete_session_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
    ) -> Result<(), SdkError> {
        let (_writer, mut reader) = self
            .send::<C>(&ClientMessage::DeleteSession {
                session_id: session_id.to_string(),
            })
            .await?;
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::SessionDeleted { .. } => return Ok(()),
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError { message, code: Some(code) });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Set the tags for a session.
    pub async fn set_tags(&self, session_id: &str, tags: Vec<String>) -> Result<(), SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.set_tags_with_codec::<JsonLineCodec>(session_id, tags).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.set_tags_with_codec::<BincodeLengthPrefixCodec>(session_id, tags).await
            }
        }
    }

    /// Set the tags for a session with a specific codec.
    pub async fn set_tags_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
        tags: Vec<String>,
    ) -> Result<(), SdkError> {
        let (_writer, mut reader) = self
            .send::<C>(&ClientMessage::SetSessionTags {
                session_id: session_id.to_string(),
                tags,
            })
            .await?;
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::SessionTagsUpdated { .. } => return Ok(()),
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError {
                        message,
                        code: Some(code),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// List all sessions from the bus.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.list_sessions_with_codec::<JsonLineCodec>().await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.list_sessions_with_codec::<BincodeLengthPrefixCodec>().await
            }
        }
    }

    /// List all sessions with a specific codec.
    pub async fn list_sessions_with_codec<C: BusCodec>(
        &self,
    ) -> Result<Vec<SessionInfo>, SdkError> {
        let (_writer, mut reader) = self.send::<C>(&ClientMessage::ListSessions).await?;
        while let Some(msg) = reader.read_msg::<ServerMessage>().await? {
            match msg {
                ServerMessage::SessionsList { sessions } => return Ok(sessions),
                ServerMessage::Error { message, code, .. } => {
                    return Err(SdkError::BusError {
                        message,
                        code: Some(code),
                    });
                }
                _ => {}
            }
        }
        Ok(vec![])
    }

    /// Fetch the full history of a session.
    pub async fn get_history(&self, session_id: &str) -> Result<Vec<Chunk>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.get_history_with_codec::<JsonLineCodec>(session_id).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.get_history_with_codec::<BincodeLengthPrefixCodec>(session_id).await
            }
        }
    }

    /// Fetch session history with a specific codec.
    pub async fn get_history_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
    ) -> Result<Vec<Chunk>, SdkError> {
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
                    return Err(SdkError::BusError {
                        message,
                        code: Some(code),
                    });
                }
                _ => {}
            }
        }
        Ok(chunks)
    }

    /// Subscribe to a session.
    pub async fn subscribe_session(
        &self,
        session_id: &str,
    ) -> Result<SessionSubscription<T::Writer>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.subscribe_session_with_codec::<JsonLineCodec>(session_id).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.subscribe_session_with_codec::<BincodeLengthPrefixCodec>(session_id).await
            }
        }
    }

    /// Subscribe to a session with a connection role (for ephemeral session lifecycle).
    pub async fn subscribe_session_with_role(
        &self,
        session_id: &str,
        role: &str,
    ) -> Result<SessionSubscription<T::Writer>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.subscribe_session_with_codec_and_role::<JsonLineCodec>(session_id, Some(role)).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.subscribe_session_with_codec_and_role::<BincodeLengthPrefixCodec>(session_id, Some(role)).await
            }
        }
    }

    /// Subscribe to a session with a specific codec.
    pub async fn subscribe_session_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
    ) -> Result<SessionSubscription<T::Writer>, SdkError> {
        self.subscribe_session_with_codec_and_role::<C>(session_id, None).await
    }

    /// Subscribe to a session with an explicit codec and an optional connection role.
    async fn subscribe_session_with_codec_and_role<C: BusCodec>(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<SessionSubscription<T::Writer>, SdkError> {
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
            codec: ClientCodec::from_str(C::NAME).unwrap_or(ClientCodec::Json),
        })
    }

    /// Subscribe to a session and return a channel receiver.
    pub async fn subscribe(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.subscribe_with_codec::<JsonLineCodec>(session_id).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.subscribe_with_codec::<BincodeLengthPrefixCodec>(session_id).await
            }
        }
    }

    /// Subscribe to a session with a connection role (for ephemeral lifecycle).
    pub async fn subscribe_with_role(
        &self,
        session_id: &str,
        role: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.subscribe_with_codec_and_role::<JsonLineCodec>(session_id, Some(role)).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.subscribe_with_codec_and_role::<BincodeLengthPrefixCodec>(session_id, Some(role)).await
            }
        }
    }

    /// Subscribe to a session with a specific codec.
    pub async fn subscribe_with_codec<C: BusCodec>(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        self.subscribe_with_codec_and_role::<C>(session_id, None)
            .await
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

    /// Subscribe to all sessions matching a filter.
    pub async fn subscribe_filtered(
        &self,
        filter: cafe_types::SubscribeFilter,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.subscribe_filtered_with_codec::<JsonLineCodec>(filter).await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.subscribe_filtered_with_codec::<BincodeLengthPrefixCodec>(filter).await
            }
        }
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

    /// Subscribe to all sessions.
    pub async fn subscribe_all(&self) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.subscribe_all_with_codec::<JsonLineCodec>().await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.subscribe_all_with_codec::<BincodeLengthPrefixCodec>().await
            }
        }
    }

    /// Subscribe to all sessions with a specific codec.
    pub async fn subscribe_all_with_codec<C: BusCodec>(
        &self,
    ) -> Result<mpsc::Receiver<ServerMessage>, SdkError> {
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

    /// Send a ping to the bus and wait for a pong.
    pub async fn ping(&self) -> Result<(), SdkError> {
        let codec = self.negotiate().await?;
        match codec {
            ClientCodec::Json => {
                self.ping_with_codec::<JsonLineCodec>().await
            }
            #[cfg(feature = "bincode-client")]
            ClientCodec::Bincode => {
                self.ping_with_codec::<BincodeLengthPrefixCodec>().await
            }
        }
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
