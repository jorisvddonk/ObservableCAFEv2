use crate::error::SdkError;
use cafe_types::{Chunk, ServerMessage};
use futures_util::{SinkExt, StreamExt};

type WsWriter =
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, tokio_tungstenite::tungstenite::Message>;

/// WebSocket client for the cafe-server session endpoint.
///
/// Wire protocol (JSON on both directions):
///
/// Server → Client:
///   {"event":"chunk","chunk":{...}}
///   {"event":"history_complete","count":0}
///
/// Client → Server:
///   {"op":"publish","chunk":{"content_type":"text","content":"hello"}}
///   {"op":"subscribe","session_id":"<new>"}
pub struct WsClient {
    writer: tokio::sync::Mutex<WsWriter>,
}

impl WsClient {
    /// Connect to a cafe-server WebSocket session endpoint.
    ///
    /// Returns a client handle and a channel receiver for incoming `ServerMessage` events.
    pub async fn connect(
        server_url: &str,
        session_id: &str,
        token: &str,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<ServerMessage>), SdkError> {
        let ws_base = server_url
            .trim_end_matches('/')
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        let url = format!("{}/api/sessions/{}/ws?token={}", ws_base, session_id, token);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;

        let (writer, reader) = ws_stream.split();

        let (tx, rx) = tokio::sync::mpsc::channel::<ServerMessage>(256);
        let sid = session_id.to_string();

        tokio::spawn(async move {
            let mut reader = reader;
            while let Some(Ok(msg)) = reader.next().await {
                if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        match val["event"].as_str() {
                            Some("chunk") => {
                                if let Ok(chunk) = serde_json::from_value(val["chunk"].clone()) {
                                    if tx.send(ServerMessage::Chunk {
                                        session_id: sid.clone(),
                                        chunk,
                                    }).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Some("history_complete") => {
                                if tx.send(ServerMessage::HistoryComplete {
                                    session_id: sid.clone(),
                                    count: val["count"].as_u64().unwrap_or(0) as usize,
                                }).await.is_err() {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok((Self { writer: tokio::sync::Mutex::new(writer) }, rx))
    }

    /// Publish a chunk to the current session.
    pub async fn publish(&self, chunk: &Chunk) -> Result<(), SdkError> {
        let payload = serde_json::to_string(&serde_json::json!({
            "op": "publish",
            "chunk": chunk,
        }))?;
        let mut writer = self.writer.lock().await;
        writer
            .send(tokio_tungstenite::tungstenite::Message::Text(payload.into()))
            .await
            .map_err(|e| SdkError::BusConnect(e.into()))?;
        Ok(())
    }
}
