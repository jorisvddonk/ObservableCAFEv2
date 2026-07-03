use crate::config::resolve_session_config;
use crate::executor::{PipelineContext, PipelineExecutor, TriggerType};
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, roles, ContentType, ServerMessage};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Subscribe to a session and route incoming chunks to the pipeline executor.
pub async fn run_session_loop(
    session_id: String,
    socket_path: String,
    executor: Arc<PipelineExecutor>,
) {
    let client = BusClient::new(&socket_path);
    let bus = Arc::new(client);

    let mut rx = match bus.subscribe(&session_id).await {
        Ok(rx) => rx,
        Err(e) => {
            warn!(
                "session_loop: failed to subscribe to session {}: {}",
                session_id, e
            );
            return;
        }
    };

    let mut history_complete = false;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                continue;
            }
            _ => continue,
        };

        // Skip transient chunks in trigger routing
        if chunk.is_transient() {
            continue;
        }

        // Wait for history replay to complete before routing
        if !history_complete {
            continue;
        }

        // Determine trigger type from the chunk
        let trigger = match () {
            // User message → fire user_message trigger
            _ if chunk.content_type == ContentType::Text && chunk.role() == Some(roles::USER) => {
                Some(TriggerType::UserMessage)
            }
            // LLM final chunk (stream_complete, non-transient, assistant) → fire llm_complete
            _ if chunk.role() == Some(roles::ASSISTANT)
                && chunk
                    .get_annotation::<bool>(keys::CHAT_STREAM_COMPLETE)
                    .unwrap_or(false) =>
            {
                Some(TriggerType::LlmComplete)
            }
            // Scheduler tick → fire scheduler_tick
            _ if chunk
                .get_annotation::<String>(keys::FLOW_SIGNAL)
                .as_deref()
                == Some("tick") =>
            {
                Some(TriggerType::SchedulerTick)
            }
            _ => None,
        };

        if let Some(trigger_type) = trigger {
            let history = match bus.get_history(&session_id).await {
                Ok(h) => h,
                Err(e) => {
                    warn!(
                        "session_loop: failed to get history for {}: {}",
                        session_id, e
                    );
                    continue;
                }
            };
            let config = resolve_session_config(&history);

            let assembled_llm_text = if trigger_type == TriggerType::LlmComplete {
                chunk.content.clone()
            } else {
                None
            };

            let ctx = PipelineContext {
                session_id: session_id.clone(),
                config,
                assembled_llm_text,
                depth: 0,
            };

            debug!(
                "session_loop: trigger={:?} session={}",
                trigger_type, session_id
            );

            if let Err(e) = executor
                .on_trigger(&trigger_type, &ctx, &bus)
                .await
            {
                error!(
                    "session_loop: pipeline error for session {}: {}",
                    session_id, e
                );
                // Publish error chunk to session (non-transient, so it appears in history)
                let err_chunk = cafe_sdk::Chunk::new_null("com.nominal.cafe-agent-runtime")
                    .with_annotation(keys::ERROR_MESSAGE, e.to_string())
                    .with_annotation("error.source", "pipeline");
                if let Err(pub_err) = bus.publish(&session_id, err_chunk).await {
                    warn!(
                        "session_loop: failed to publish error chunk for {}: {}",
                        session_id, pub_err
                    );
                }
            }
        }
    }

    info!("session_loop: session {} disconnected", session_id);
}
