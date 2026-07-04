use crate::config::SessionConfig;
use crate::tool_detector;
use crate::tool_executor;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcRequest, JsonRpcResponse, SdkError, ServerMessage, StepDef};
use std::collections::VecDeque;
use std::time::Duration;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Step type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerType {
    UserMessage,
    LlmComplete,
    SchedulerTick,
    StepComplete(String),
}

impl TriggerType {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "user_message" => TriggerType::UserMessage,
            "llm_complete" => TriggerType::LlmComplete,
            "scheduler_tick" => TriggerType::SchedulerTick,
            other => {
                if let Some(id) = other.strip_prefix("step_complete:") {
                    TriggerType::StepComplete(id.to_string())
                } else {
                    warn!("executor: unknown trigger type '{}', treating as user_message", s);
                    TriggerType::UserMessage
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum StepType {
    BuiltIn(BuiltInEvaluator),
    Rpc(String),
}

impl StepType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "role-annotator" => StepType::BuiltIn(BuiltInEvaluator::RoleAnnotator),
            "trust-filter" => StepType::BuiltIn(BuiltInEvaluator::TrustFilter),
            "tool-detector" => StepType::BuiltIn(BuiltInEvaluator::ToolDetector),
            "tool-executor" => StepType::BuiltIn(BuiltInEvaluator::ToolExecutor),
            other => StepType::Rpc(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum BuiltInEvaluator {
    RoleAnnotator,
    TrustFilter,
    ToolDetector,
    ToolExecutor,
}

// ---------------------------------------------------------------------------
// Pipeline context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PipelineContext {
    pub session_id: String,
    pub config: SessionConfig,
    pub assembled_llm_text: Option<String>,
    /// Original user message text (for user_message triggers)
    pub user_text: Option<String>,
    /// Current recursion depth (for step_complete chaining limit).
    pub depth: u32,
}

// ---------------------------------------------------------------------------
// Pipeline error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("RPC step '{step_id}' timed out waiting for call_id={call_id}")]
    Timeout {
        step_id: String,
        call_id: String,
    },

    #[error("RPC step '{step_id}' returned error: [{code}] {message}")]
    RpcError {
        step_id: String,
        code: i32,
        message: String,
    },

    #[error("Bus error in step '{step_id}': {source}")]
    Bus {
        step_id: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("SDK error: {0}")]
    Sdk(#[from] SdkError),

    #[error("I/O error: {0}")]
    Io(#[source] anyhow::Error),
}

// ---------------------------------------------------------------------------
// Pipeline executor
// ---------------------------------------------------------------------------

/// Holds the ordered list of steps and dispatches them on trigger events.
#[derive(Clone)]
pub struct PipelineExecutor {
    steps: Vec<StepDef>,
    rpc_timeout: Duration,
    max_depth: u32,
}

impl PipelineExecutor {
    pub fn new(steps: Vec<StepDef>, rpc_timeout: Duration, max_depth: u32) -> Self {
        Self {
            steps,
            rpc_timeout,
            max_depth,
        }
    }

    /// Called when a triggering event occurs. Uses a work queue to handle
    /// StepComplete chains without recursion, respecting max_depth.
    pub async fn on_trigger(
        &self,
        event: &TriggerType,
        context: &PipelineContext,
        bus: &BusClient,
    ) -> Result<(), PipelineError> {
        let mut queue = VecDeque::new();
        queue.push_back((event.clone(), context.clone()));

        while let Some((ev, ctx)) = queue.pop_front() {
            // Skip depth-limited StepComplete chains
            if matches!(&ev, TriggerType::StepComplete(_)) && ctx.depth >= self.max_depth {
                warn!(
                    "executor: depth limit ({}) reached; skipping step_complete chain",
                    self.max_depth
                );
                continue;
            }

            let eligible: Vec<&StepDef> = self
                .steps
                .iter()
                .filter(|s| trigger_matches(&s.trigger, &ev))
                .filter(|s| is_enabled(s, &ctx.config))
                .collect();

            if eligible.is_empty() {
                continue;
            }

            // Run built-in steps
            for step in &eligible {
                if let StepType::BuiltIn(eval) = StepType::from_str(&step.step_type) {
                    match eval {
                        BuiltInEvaluator::RoleAnnotator => {}
                        BuiltInEvaluator::TrustFilter => {}
                        BuiltInEvaluator::ToolDetector => {
                            if let Some(ref text) = ctx.assembled_llm_text {
                                let (_, calls) = tool_detector::detect(text);
                                for call in &calls {
                                    let chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                                        .with_annotation(keys::TOOL_CALL, call);
                                    if let Err(e) = bus.publish(&ctx.session_id, chunk).await {
                                        warn!("executor: failed to publish tool.call: {}", e);
                                    }
                                }
                                if !calls.is_empty() {
                                    info!(
                                        "executor: detected {} tool call(s) in session {}",
                                        calls.len(), ctx.session_id
                                    );
                                }
                            }
                        }
                        BuiltInEvaluator::ToolExecutor => {
                            match bus.get_history(&ctx.session_id).await {
                                Ok(history) => {
                                    let calls: Vec<_> = history.iter().rev().filter_map(|c| c.as_tool_call()).collect();
                                    if !calls.is_empty() {
                                        for call in &calls {
                                            if let Err(e) = tool_executor::execute(call, &ctx.session_id, bus).await {
                                                warn!("executor: tool_executor error: {}", e);
                                            }
                                        }
                                        // Queue StepComplete so follow-up steps fire
                                        queue.push_back((
                                            TriggerType::StepComplete(step.id.clone()),
                                            PipelineContext {
                                                depth: ctx.depth + 1,
                                                ..context.clone()
                                            },
                                        ));
                                    }
                                }
                                Err(e) => warn!("executor: failed to get history for tool execution: {}", e),
                            }
                        }
                    }
                }
            }

            // Dispatch RPC steps and queue their StepComplete triggers
            for step in &eligible {
                if let StepType::Rpc(namespace) = StepType::from_str(&step.step_type) {
                    let result = self
                        .dispatch_rpc(step, &ctx, bus)
                        .await;
                    match result {
                        Ok(Some(follow_up)) => {
                            queue.push_back(follow_up);
                        }
                        Ok(None) => {}
                        Err(e) => return Err(e),
                    }
                    let _ = namespace;
                }
            }
        }

        Ok(())
    }

}

/// Check if a step's `trigger` string matches a `TriggerType` event.
fn trigger_matches(step_trigger: &str, event: &TriggerType) -> bool {
    match event {
        TriggerType::UserMessage => step_trigger == "user_message",
        TriggerType::LlmComplete => step_trigger == "llm_complete",
        TriggerType::SchedulerTick => step_trigger == "scheduler_tick",
        TriggerType::StepComplete(id) => {
            step_trigger == &format!("step_complete:{}", id)
                || step_trigger == "step_complete"
        }
    }
}

/// Check if a step is enabled given the resolved session config.
fn is_enabled(step: &StepDef, config: &SessionConfig) -> bool {
    match &step.enabled_if {
        None => true,
        Some(key) => config
            .extra
            .get(key.as_str())
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

/// Build RPC params for a given namespace and pipeline context.
fn build_rpc_params(namespace: &str, ctx: &PipelineContext) -> serde_json::Value {
    match namespace {
        "llm" => serde_json::json!({
            "session_id": ctx.session_id,
        }),
        "tts" => serde_json::json!({
            "text":    ctx.assembled_llm_text.as_deref().unwrap_or(""),
            "profile": ctx.config.tts_profile,
            "engine":  ctx.config.tts_engine,
        }),
        "comfy" => {
            let mut params = serde_json::json!({
                "prompt": ctx.assembled_llm_text.as_deref().unwrap_or(""),
            });
            if let Some(path) = &ctx.config.comfy_workflow_path {
                params["workflow_path"] = serde_json::Value::String(path.clone());
            }
            if let Some(node) = &ctx.config.comfy_workflow_input_node {
                params["input_node"] = serde_json::Value::String(node.clone());
            }
            params
        }
        _ => {
            let mut params = serde_json::json!({
                "session_id": ctx.session_id,
            });
            let text = ctx.assembled_llm_text.as_deref()
                .or_else(|| ctx.user_text.as_deref());
            if let Some(text) = text {
                params["text"] = serde_json::Value::String(text.to_string());
            }
            params
        }
    }
}

impl PipelineExecutor {
    /// Dispatch an RPC step: publish transient request, await response,
    /// return an optional StepComplete follow-up for the work queue.
    async fn dispatch_rpc(
        &self,
        step: &&StepDef,
        context: &PipelineContext,
        bus: &BusClient,
    ) -> Result<Option<(TriggerType, PipelineContext)>, PipelineError> {
        let namespace = &step.step_type;
        let params = build_rpc_params(namespace, context);
        let method = format!("{}.invoke", namespace);
        let request = JsonRpcRequest::new(&method, params);
        let call_id = request.id.clone();

        info!(
            "executor: dispatching RPC {method} call_id={call_id} step={} session={}",
            step.id, context.session_id
        );

        bus.get_history(&context.session_id)
            .await
            .map_err(|e| PipelineError::Bus {
                step_id: step.id.clone(),
                source: e.into(),
            })?;

        let req_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
            .with_annotation(keys::JSONRPC_REQUEST, &request)
            .as_transient()
            .with_retain(60);
        bus.publish(&context.session_id, req_chunk)
            .await
            .map_err(|e| PipelineError::Bus {
                step_id: step.id.clone(),
                source: e.into(),
            })?;

        let mut rx = bus.subscribe(&context.session_id).await.map_err(|e| {
            PipelineError::Bus {
                step_id: step.id.clone(),
                source: e.into(),
            }
        })?;

        let response: JsonRpcResponse =
            tokio::time::timeout(self.rpc_timeout, async {
                loop {
                    match rx.recv().await {
                        Some(ServerMessage::Chunk { chunk, .. }) => {
                            if chunk.is_rpc_response_for(&call_id) {
                                return chunk.as_rpc_response().ok_or_else(|| {
                                    PipelineError::Io(anyhow::anyhow!(
                                        "failed to deserialize RPC response for call {}",
                                        call_id
                                    ))
                                });
                            }
                        }
                        Some(_) => continue,
                        None => {
                            return Err(PipelineError::Io(anyhow::anyhow!(
                                "bus disconnected while waiting for RPC response"
                            )));
                        }
                    }
                }
            })
            .await
            .map_err(|_| PipelineError::Timeout {
                step_id: step.id.clone(),
                call_id: call_id.clone(),
            })?
            .map_err(|e| PipelineError::Bus {
                step_id: step.id.clone(),
                source: e.into(),
            })?;

        if response.is_ok() {
            info!(
                "executor: RPC {method} succeeded call_id={call_id} step={} session={}",
                step.id, context.session_id
            );
        } else {
            let err = response.error.unwrap();
            return Err(PipelineError::RpcError {
                step_id: step.id.clone(),
                code: err.code,
                message: err.message,
            });
        }

        // Return StepComplete follow-up for the work queue
        let follow_up = (TriggerType::StepComplete(step.id.clone()), PipelineContext {
            depth: context.depth + 1,
            ..context.clone()
        });

        Ok(Some(follow_up))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_type_from_str_user_message() {
        assert_eq!(TriggerType::from_str("user_message"), TriggerType::UserMessage);
    }

    #[test]
    fn trigger_type_from_str_llm_complete() {
        assert_eq!(TriggerType::from_str("llm_complete"), TriggerType::LlmComplete);
    }

    #[test]
    fn trigger_type_from_str_scheduler_tick() {
        assert_eq!(TriggerType::from_str("scheduler_tick"), TriggerType::SchedulerTick);
    }

    #[test]
    fn trigger_type_from_str_step_complete() {
        assert_eq!(
            TriggerType::from_str("step_complete:tts"),
            TriggerType::StepComplete("tts".into())
        );
    }

    #[test]
    fn step_type_from_str_builtin() {
        assert!(matches!(StepType::from_str("trust-filter"), StepType::BuiltIn(BuiltInEvaluator::TrustFilter)));
        assert!(matches!(StepType::from_str("tool-detector"), StepType::BuiltIn(BuiltInEvaluator::ToolDetector)));
        assert!(matches!(StepType::from_str("tool-executor"), StepType::BuiltIn(BuiltInEvaluator::ToolExecutor)));
        assert!(matches!(StepType::from_str("role-annotator"), StepType::BuiltIn(BuiltInEvaluator::RoleAnnotator)));
    }

    #[test]
    fn step_type_from_str_rpc() {
        assert!(matches!(StepType::from_str("llm"), StepType::Rpc(_)));
        assert!(matches!(StepType::from_str("tts"), StepType::Rpc(_)));
        assert!(matches!(StepType::from_str("comfy"), StepType::Rpc(_)));
        assert!(matches!(StepType::from_str("rss-fetch"), StepType::Rpc(_)));
    }

    #[test]
    fn trigger_matches_user_message() {
        assert!(trigger_matches("user_message", &TriggerType::UserMessage));
        assert!(!trigger_matches("llm_complete", &TriggerType::UserMessage));
    }

    #[test]
    fn trigger_matches_step_complete() {
        assert!(trigger_matches("step_complete:tts", &TriggerType::StepComplete("tts".into())));
        assert!(!trigger_matches("step_complete:tts", &TriggerType::StepComplete("comfy".into())));
    }

    #[test]
    fn is_enabled_no_field() {
        let step = StepDef {
            id: "test".into(),
            step_type: "llm".into(),
            trigger: "user_message".into(),
            enabled_if: None,
        };
        let config = SessionConfig::default();
        assert!(is_enabled(&step, &config));
    }

    #[test]
    fn is_enabled_true() {
        let step = StepDef {
            id: "test".into(),
            step_type: "tts".into(),
            trigger: "llm_complete".into(),
            enabled_if: Some("config.tts.enabled".into()),
        };
        let mut config = SessionConfig::default();
        config.extra.insert("config.tts.enabled".into(), serde_json::json!(true));
        assert!(is_enabled(&step, &config));
    }

    #[test]
    fn is_enabled_false() {
        let step = StepDef {
            id: "test".into(),
            step_type: "tts".into(),
            trigger: "llm_complete".into(),
            enabled_if: Some("config.tts.enabled".into()),
        };
        let config = SessionConfig::default();
        assert!(!is_enabled(&step, &config));
    }
}
