use crate::backends::{LlmBackend, LlmParams};
use crate::context::{build_messages, extract_config};
use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, roles, Chunk, ClientMessage, ContentType, JsonRpcResponse, ServerMessage};
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

/// Main evaluation loop: subscribe to a session, respond to llm.invoke RPC requests.
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
                            continue;
                        }
                    }
                }

                // Handle llm.invoke RPC requests
                if let Some(rpc) = chunk.as_rpc_request() {
                    if rpc.method == "llm.invoke" {
                        let bus = BusClient::new(&socket_path);
                        let history = match bus.get_history(&session_id).await {
                            Ok(h) => h,
                            Err(e) => {
                                warn!("cafe-llm: failed to get history for {}: {}", session_id, e);
                                continue;
                            }
                        };

                        let cfg = extract_config(&history);
                        let model = cfg.model.clone().unwrap_or_else(|| default_model.clone());
                        let messages = build_messages(&history, cfg.system_prompt.as_deref());

                        let params = LlmParams {
                            model: model.clone(),
                            temperature: cfg.temperature,
                            max_tokens: cfg.max_tokens,
                        };

                        info!(
                            "cafe-llm: handling llm.invoke call_id={} session={}",
                            rpc.id, session_id
                        );

                        let _ = abort_tx.send(false);

                        handle_llm_response(
                            session_id.clone(),
                            &mut writer,
                            &backend,
                            &default_model,
                            messages,
                            params,
                            &rpc.id,
                        )
                        .await;
                    }
                }
            }

            ServerMessage::HistoryComplete { .. } => {
                info!("cafe-llm: history replay complete for session {}", session_id);
            }

            _ => {}
        }
    }

    Ok(())
}

async fn handle_llm_response(
    session_id: String,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    backend: &Arc<dyn LlmBackend>,
    _default_model: &str,
    messages: Vec<crate::backends::LlmMessage>,
    params: LlmParams,
    call_id: &str,
) {
    let model = params.model.clone();

    let mut token_stream = match backend.complete(messages, &params).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("cafe-llm: backend error: {}", e);
            let err_chunk = Chunk::new_null("com.nominal.cafe-llm")
                .with_annotation(keys::ERROR_MESSAGE, e.to_string())
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
                .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
                .with_annotation(keys::CHAT_FINISH_REASON, "error");
            publish_chunk(writer, &session_id, err_chunk).await;

            let rpc_err = JsonRpcResponse::err(call_id, -1, &e.to_string());
            let err_resp_chunk = Chunk::new_null("com.nominal.cafe-llm")
                .with_annotation(keys::JSONRPC_RESPONSE, &rpc_err)
                .as_transient()
                .with_retain(60);
            publish_chunk(writer, &session_id, err_resp_chunk).await;
            return;
        }
    };

    // Signal streaming start immediately (before first token)
    let start_chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CHAT_IS_STREAMING, true)
        .as_transient()
        .with_retain(5);
    publish_chunk(writer, &session_id, start_chunk).await;

    let mut finish_reason = "stop".to_string();
    let (_abort_tx, mut abort_rx) = watch::channel(false);
    let mut full_response = String::new();
    let mut token_ids: Vec<String> = Vec::new();

    loop {
        tokio::select! {
            token = token_stream.next() => {
                match token {
                    Some(Ok(text)) => {
                        full_response.push_str(&text);
                        let token_chunk = Chunk::new_text(&text, "com.nominal.cafe-llm")
                            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
                            .with_annotation(keys::CHAT_IS_STREAMING, true)
                            .as_transient();
                        token_ids.push(token_chunk.id.clone());
                        publish_chunk(writer, &session_id, token_chunk).await;
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

    if !full_response.is_empty() {
        let response_chunk = Chunk::new_text(&full_response, "com.nominal.cafe-llm")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
            .with_annotation(keys::CHAT_IS_STREAMING, true)
            .with_annotation(keys::CHAT_MODEL, &model);
        publish_chunk(writer, &session_id, response_chunk).await;
    }

    // Publish tombstone for all transient token chunks (before stream_complete
    // so SSE/UI consumers receive it before the stream ends)
    let tombstone = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::FLOW_TOMBSTONE, &token_ids)
        .as_transient();
    publish_chunk(writer, &session_id, tombstone).await;

    // Publish stream_complete with assistant role so LlmComplete trigger fires
    let done_chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
        .with_annotation(keys::CHAT_FINISH_REASON, &finish_reason);
    publish_chunk(writer, &session_id, done_chunk).await;

    // Publish RPC response so the pipeline's dispatch_rpc completes
    let rpc_resp = JsonRpcResponse::ok(call_id, serde_json::json!({"status": "ok"}));
    let rpc_chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::JSONRPC_RESPONSE, &rpc_resp)
        .as_transient()
        .with_retain(60);
    publish_chunk(writer, &session_id, rpc_chunk).await;
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
