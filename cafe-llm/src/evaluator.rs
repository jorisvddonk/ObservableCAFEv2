use crate::backends::{LlmBackend, LlmMessage, LlmParams};
use crate::context::{build_messages, extract_config};
use anyhow::Result;
use async_trait::async_trait;
use cafe_sdk::bus::{BusClient, SessionSubscription};
use cafe_sdk::{keys, roles, Chunk, ContentType, JsonRpcResponse, ServerMessage};
use futures_util::StreamExt;
use std::sync::Arc;
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

/// Sink for streamed chunks. Implemented for the real bus subscription so
/// tokens are published live, and for a collecting sink in tests.
#[async_trait]
trait StreamSink {
    async fn emit(&mut self, chunk: Chunk);
}

#[async_trait]
impl StreamSink for SessionSubscription {
    async fn emit(&mut self, chunk: Chunk) {
        let _ = self.publish(chunk).await;
    }
}

/// Main evaluation loop: subscribe to a session, respond to llm.invoke RPC requests.
pub async fn run_session(
    session_id: String,
    socket_path: String,
    backend: Arc<dyn LlmBackend>,
    default_model: String,
) -> Result<()> {
    let client = BusClient::unix(&socket_path);
    let mut sub = client.subscribe_session(&session_id).await?;

    let (abort_tx, _abort_rx) = watch::channel(false);

    // Concurrently watch for abort flow signals so an in-flight generation can
    // actually be interrupted (run_session would otherwise be blocked inside
    // handle_llm_response and unable to receive bus messages).
    let abort_tx_for_listener = abort_tx.clone();
    let mut abort_listener = client.subscribe_session(&session_id).await?;
    let listener_session = session_id.clone();
    tokio::spawn(async move {
        while let Some(msg) = abort_listener.rx.recv().await {
            if let ServerMessage::Chunk {
                session_id: sid,
                chunk,
            } = msg
            {
                if sid == listener_session && chunk.content_type == ContentType::Null {
                    if let Some(signal) = chunk.get_annotation::<String>(keys::CAFE_FLOW_SIGNAL) {
                        if signal == "abort" {
                            let _ = abort_tx_for_listener.send(true);
                        }
                    }
                }
            }
        }
    });

    while let Some(msg) = sub.rx.recv().await {
        match msg {
            ServerMessage::Chunk {
                session_id: sid,
                chunk,
            } if sid == session_id => {
                // Check for abort signal
                if chunk.content_type == ContentType::Null {
                    if let Some(signal) = chunk.get_annotation::<String>(keys::CAFE_FLOW_SIGNAL) {
                        if signal == "abort" {
                            let _ = abort_tx.send(true);
                            continue;
                        }
                    }
                }

                // Handle llm.invoke RPC requests
                if let Some(rpc) = chunk.as_rpc_request() {
                    if rpc.method == "llm.invoke" {
                        let history = match client.get_history(&session_id).await {
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

                        // Reset the abort flag before starting a fresh generation so a
                        // previous abort does not immediately cancel this one.
                        let _ = abort_tx.send(false);

                        handle_llm_response(
                            &mut sub,
                            &backend,
                            &default_model,
                            messages,
                            params,
                            &rpc.id,
                            abort_tx.subscribe(),
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
    sub: &mut SessionSubscription,
    backend: &Arc<dyn LlmBackend>,
    _default_model: &str,
    messages: Vec<LlmMessage>,
    params: LlmParams,
    call_id: &str,
    mut abort_rx: watch::Receiver<bool>,
) {
    let model = params.model.clone();

    let mut token_stream = match backend.complete(messages, &params).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("cafe-llm: backend error: {}", e);
            let err_chunk = Chunk::new_null("com.nominal.cafe-llm")
                .with_annotation(keys::CAFE_ERROR_MESSAGE, e.to_string())
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
                .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
                .with_annotation(keys::CHAT_FINISH_REASON, "error");
            let _ = sub.publish(err_chunk).await;

            let rpc_err = JsonRpcResponse::err(call_id, -1, &e.to_string());
            let err_resp_chunk = Chunk::new_null("com.nominal.cafe-llm")
                .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &rpc_err)
                .as_transient()
                .with_retain(60);
            let _ = sub.publish(err_resp_chunk).await;
            return;
        }
    };

    // Signal streaming start immediately (before first token)
    let start_chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CHAT_IS_STREAMING, true)
        .as_transient()
        .with_retain(5);
    let _ = sub.publish(start_chunk).await;

    // Drive the token stream, sharing the SAME abort channel that run_session
    // (and its listener task) signal on. This is the bug fix: previously this
    // loop watched a freshly-created, disconnected watch channel that nothing
    // ever signalled, so streaming could never be interrupted.
    let (finish_reason, full_response, token_ids) =
        run_stream_loop(&mut token_stream, &mut abort_rx, sub).await;

    if !full_response.is_empty() {
        let response_chunk = Chunk::new_text(&full_response, "com.nominal.cafe-llm")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
            .with_annotation(keys::CHAT_IS_STREAMING, true)
            .with_annotation(keys::CHAT_MODEL, &model);
        let _ = sub.publish(response_chunk).await;
    }

    // Publish tombstone for all transient token chunks
    let tombstone = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::CAFE_FLOW_TOMBSTONE, &token_ids)
        .as_transient();
    let _ = sub.publish(tombstone).await;

    // Publish stream_complete with assistant role so LlmComplete trigger fires
    let done_chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
        .with_annotation(keys::CHAT_FINISH_REASON, &finish_reason);
    let _ = sub.publish(done_chunk).await;

    // Publish RPC response so the pipeline's dispatch_rpc completes
    let rpc_resp = JsonRpcResponse::ok(call_id, serde_json::json!({"status": "ok"}));
    let rpc_chunk = Chunk::new_null("com.nominal.cafe-llm")
        .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &rpc_resp)
        .as_transient()
        .with_retain(60);
    let _ = sub.publish(rpc_chunk).await;
}

/// Consume a token stream until it ends or the shared `abort_rx` is signalled.
/// Returns `(finish_reason, full_response_text, token_chunk_ids)`.
async fn run_stream_loop<S, Sink>(
    token_stream: &mut S,
    abort_rx: &mut watch::Receiver<bool>,
    sink: &mut Sink,
) -> (String, String, Vec<String>)
where
    S: StreamExt<Item = Result<String>> + Unpin,
    Sink: StreamSink + Send,
{
    let mut finish_reason = "stop".to_string();
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
                        sink.emit(token_chunk).await;
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

    (finish_reason, full_response, token_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream::{self, BoxStream, StreamExt};
    use tokio::sync::watch;
    use tokio::time::{timeout, Duration};

    /// A fake stream that yields the given tokens then hangs forever
    /// (never terminates), simulating a slow/never-ending generation.
    fn hanging_stream(tokens: Vec<String>) -> BoxStream<'static, Result<String>> {
        let iter = stream::iter(tokens.into_iter().map(|t| Ok(t)));
        iter.chain(stream::pending()).boxed()
    }

    /// Test sink that records emitted chunks without touching the bus.
    struct CollectingSink {
        chunks: Vec<Chunk>,
    }

    #[async_trait]
    impl StreamSink for CollectingSink {
        async fn emit(&mut self, chunk: Chunk) {
            self.chunks.push(chunk);
        }
    }

    // Reproduces the original bug: when the loop watches a *disconnected* abort
    // channel (as the pre-fix code did, creating its own watch::channel), an
    // abort signal sent on an unrelated sender must NOT stop generation.
    #[tokio::test]
    async fn disconnected_abort_channel_does_not_stop_generation() {
        // The receiver the loop watches — never connected to a live sender that
        // we can signal.
        let (_tx, mut abort_rx) = watch::channel(false);
        let mut stream = hanging_stream(vec!["hello ".to_string()]);

        // A separate, unrelated sender that the loop never observes.
        let (real_tx, _real_rx) = watch::channel(false);
        let signal = real_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = signal.send(true);
        });

        let mut sink = CollectingSink { chunks: Vec::new() };
        let result = timeout(
            Duration::from_millis(400),
            run_stream_loop(&mut stream, &mut abort_rx, &mut sink),
        )
        .await;

        assert!(
            result.is_err(),
            "generation must NOT stop when the abort channel is disconnected (bug)"
        );
    }

    // Fix verification: when the loop watches the SAME abort channel that
    // run_session signals on, an abort sent mid-generation must stop it.
    #[tokio::test]
    async fn shared_abort_channel_stops_generation() {
        let (abort_tx, mut abort_rx) = watch::channel(false);
        let mut stream = hanging_stream(vec!["hello ".to_string(), "world".to_string()]);

        let signal = abort_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = signal.send(true);
        });

        let mut sink = CollectingSink { chunks: Vec::new() };
        let (finish_reason, full_response, token_ids) = timeout(
            Duration::from_secs(2),
            run_stream_loop(&mut stream, &mut abort_rx, &mut sink),
        )
        .await
        .expect("generation should stop on abort");

        assert_eq!(finish_reason, "abort");
        // Tokens received before the abort must have been emitted.
        assert_eq!(full_response, "hello world");
        assert_eq!(token_ids.len(), 2);
        // The abort must fire before the (never-ending) stream would complete,
        // i.e. we did not wait for the whole stream.
        assert!(sink.chunks.len() >= 2);
    }
}
