use crate::backends::{LlmBackend, LlmParams};
use crate::context::{build_messages, extract_config};
use anyhow::Result;
use cafe_types::{keys, roles, Chunk, ClientMessage, ContentType, ServerMessage};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::watch;
use tracing::{error, info, warn};

/// Manages LLM evaluation for a single session.
#[allow(dead_code)]
pub struct SessionEvaluator {
    session_id: String,
    history: Vec<Chunk>,
    abort_tx: watch::Sender<bool>,
}

impl SessionEvaluator {
    #[allow(dead_code)]
    pub fn new(session_id: String) -> Self {
        let (abort_tx, _) = watch::channel(false);
        Self {
            session_id,
            history: Vec::new(),
            abort_tx,
        }
    }

    #[allow(dead_code)]
    pub fn push_chunk(&mut self, chunk: Chunk) {
        self.history.push(chunk);
    }

    #[allow(dead_code)]
    pub fn abort(&self) {
        let _ = self.abort_tx.send(true);
    }

    #[allow(dead_code)]
    pub fn abort_receiver(&self) -> watch::Receiver<bool> {
        self.abort_tx.subscribe()
    }
}

/// Main evaluation loop: subscribe to a session, call LLM on user messages.
pub async fn run_session(
    session_id: String,
    socket_path: String,
    backend: Arc<dyn LlmBackend>,
    default_model: String,
) -> Result<()> {
    let stream = UnixStream::connect(&socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    // Subscribe to the session
    let sub = serde_json::to_string(&ClientMessage::Subscribe {
        session_id: session_id.clone(),
    })? + "\n";
    writer.write_all(sub.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    let mut history: Vec<Chunk> = Vec::new();
    let mut history_complete = false;

    // Abort channel for in-flight requests
    let (abort_tx, _abort_rx) = watch::channel(false);

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("cafe-llm: invalid message: {}", e);
                continue;
            }
        };

        match msg {
            ServerMessage::Chunk {
                session_id: sid,
                chunk,
            } if sid == session_id => {
                // Check for abort signal
                if chunk.content_type == ContentType::Null {
                    if let Some(signal) = chunk.get_annotation::<String>(keys::FLOW_SIGNAL) {
                        if signal == "abort" {
                            let _ = abort_tx.send(true);
                            history.push(chunk);
                            continue;
                        }
                    }
                }

                history.push(chunk.clone());

                // Only process user messages after history replay is done
                if !history_complete {
                    continue;
                }

                // Only respond to user text chunks
                if chunk.content_type != ContentType::Text {
                    continue;
                }
                if chunk.role() != Some(roles::USER) {
                    continue;
                }

                // Extract config from history
                let cfg = extract_config(&history);
                let model = cfg.model.clone().unwrap_or_else(|| default_model.clone());
                let messages = build_messages(&history, cfg.system_prompt.as_deref());

                let params = LlmParams {
                    model: model.clone(),
                    temperature: cfg.temperature,
                    max_tokens: cfg.max_tokens,
                };

                // Reset abort signal
                let _ = abort_tx.send(false);
                let mut abort_rx = abort_tx.subscribe();

                // Stream LLM response
                match backend.complete(messages, &params).await {
                    Ok(mut token_stream) => {
                        let mut finish_reason = "stop".to_string();

                        loop {
                            tokio::select! {
                                token = token_stream.next() => {
                                    match token {
                                        Some(Ok(text)) => {
                                            let response_chunk = Chunk::new_text(text, "com.nominal.cafe-llm")
                                                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
                                                .with_annotation(keys::CHAT_IS_STREAMING, true)
                                                .with_annotation(keys::CHAT_MODEL, &model);

                                            publish_chunk(&mut writer, &session_id, response_chunk).await;
                                        }
                                        Some(Err(e)) => {
                                            error!("cafe-llm: stream error: {}", e);
                                            finish_reason = "error".to_string();
                                            break;
                                        }
                                        None => break,
                                    }
                                }
                                _ = abort_rx.changed() => {
                                    if *abort_rx.borrow() {
                                        finish_reason = "abort".to_string();
                                        break;
                                    }
                                }
                            }
                        }

                        // Send stream_complete null chunk
                        let done_chunk = Chunk::new_null("com.nominal.cafe-llm")
                            .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
                            .with_annotation(keys::CHAT_FINISH_REASON, finish_reason);
                        publish_chunk(&mut writer, &session_id, done_chunk).await;
                    }
                    Err(e) => {
                        error!("cafe-llm: backend error: {}", e);
                        let err_chunk = Chunk::new_null("com.nominal.cafe-llm")
                            .with_annotation(keys::ERROR_MESSAGE, e.to_string())
                            .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
                            .with_annotation(keys::CHAT_FINISH_REASON, "error");
                        publish_chunk(&mut writer, &session_id, err_chunk).await;
                    }
                }
            }

            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                info!("cafe-llm: history replay complete for session {}", session_id);
            }

            _ => {}
        }
    }

    Ok(())
}

async fn publish_chunk(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    session_id: &str,
    chunk: Chunk,
) {
    let msg = ClientMessage::Publish {
        session_id: session_id.to_string(),
        chunk,
    };
    if let Ok(mut json) = serde_json::to_string(&msg) {
        json.push('\n');
        if let Err(e) = writer.write_all(json.as_bytes()).await {
            error!("cafe-llm: write error: {}", e);
        }
    }
}
