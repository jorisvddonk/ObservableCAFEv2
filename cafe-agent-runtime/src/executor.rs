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
            "mcp" => StepType::BuiltIn(BuiltInEvaluator::Mcp),
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
    Mcp,
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
                        BuiltInEvaluator::Mcp => {
                            // No-op: MCP tools are handled by cafe-mcp-client
                            // via tool.call/tool.result on the session bus.
                        }
                        BuiltInEvaluator::ToolDetector => {
                            if let Some(ref text) = ctx.assembled_llm_text {
                                let (_, calls) = tool_detector::detect(text);
                                for call in &calls {
                                    let chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                                        .with_annotation(keys::CAFE_TOOL_CALL, call);
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
        "stt" => {
            // Scan session history for the most recent binary_ref chunk with chat.role=user
            // and pass its ID + mime_type so cafe-stt can find read credentials and transcribe.
            let mut params = serde_json::json!({
                "session_id": ctx.session_id,
            });
            // History is fetched by the caller before dispatch_rpc — we don't have it here.
            // Instead, the RPC handler (cafe-stt) will scan history itself.
            // We just need to ensure the binary_ref_id is available somehow.
            // The simplest fix: pass the entire history's binary_ref info via the session.
            // For now, cafe-stt handles scanning. This requires the binary_ref to be
            // non-transient so cafe-stt can find it in history.
            params
        }
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
            .with_annotation(keys::CAFE_JSONRPC_REQUEST, &request)
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

    // ── Property-based tests (proptest) ──

    use proptest::prelude::*;

    fn arb_annotation_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn arb_trigger_type() -> impl Strategy<Value = TriggerType> {
        prop_oneof![
            Just(TriggerType::UserMessage),
            Just(TriggerType::LlmComplete),
            Just(TriggerType::SchedulerTick),
            ".{0,20}".prop_map(TriggerType::StepComplete),
        ]
    }

    fn arb_builtin_evaluator() -> impl Strategy<Value = BuiltInEvaluator> {
        prop_oneof![
            Just(BuiltInEvaluator::RoleAnnotator),
            Just(BuiltInEvaluator::TrustFilter),
            Just(BuiltInEvaluator::ToolDetector),
            Just(BuiltInEvaluator::ToolExecutor),
            Just(BuiltInEvaluator::Mcp),
        ]
    }

    fn arb_session_config() -> impl Strategy<Value = SessionConfig> {
        prop::collection::hash_map("[a-z._-]{1,30}", arb_annotation_value(), 0..5)
            .prop_map(|extra| SessionConfig { extra, ..SessionConfig::default() })
    }

    fn arb_pipeline_context() -> impl Strategy<Value = PipelineContext> {
        (
            ".{0,20}",
            arb_session_config(),
            proptest::option::of(".{0,100}"),
            proptest::option::of(".{0,100}"),
            any::<u32>(),
        ).prop_map(|(session_id, config, assembled_llm_text, user_text, depth)| {
            PipelineContext {
                session_id,
                config,
                assembled_llm_text,
                user_text,
                depth,
            }
        })
    }

    fn arb_namespace() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("llm".to_string()),
            Just("tts".to_string()),
            Just("stt".to_string()),
            Just("comfy".to_string()),
            ".{0,20}".prop_map(|s| s),
        ]
    }

    fn run_proptest<S: proptest::strategy::Strategy<Value = V>, V: std::fmt::Debug>(
        strategy: S,
        test: fn(V),
    ) {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner.run(&strategy, |v| { test(v); Ok(()) }).unwrap();
    }

    #[test]
    fn trigger_type_from_str_never_panics() {
        run_proptest(".{0,50}".prop_map(|s: String| s), |s: String| {
            let _ = TriggerType::from_str(&s);
        });
    }

    #[test]
    fn trigger_type_step_complete_roundtrip() {
        run_proptest(
            ".{0,20}",
            |id: String| {
                let prefixed = format!("step_complete:{}", id);
                assert_eq!(
                    TriggerType::from_str(&prefixed),
                    TriggerType::StepComplete(id)
                );
            },
        );
    }

    #[test]
    fn step_type_from_str_never_panics() {
        run_proptest(".{0,50}".prop_map(|s: String| s), |s: String| {
            let _ = StepType::from_str(&s);
        });
    }

    #[test]
    fn step_type_builtin_roundtrip() {
        run_proptest(arb_builtin_evaluator(), |eval: BuiltInEvaluator| {
            let s = match &eval {
                BuiltInEvaluator::RoleAnnotator => "role-annotator",
                BuiltInEvaluator::TrustFilter => "trust-filter",
                BuiltInEvaluator::ToolDetector => "tool-detector",
                BuiltInEvaluator::ToolExecutor => "tool-executor",
                BuiltInEvaluator::Mcp => "mcp",
            };
            let result = StepType::from_str(s);
            assert!(matches!(result, StepType::BuiltIn(ref e) if std::mem::discriminant(e) == std::mem::discriminant(&eval)));
        });
    }

    #[test]
    fn trigger_matches_wildcard_step_complete() {
        run_proptest(
            ".{0,20}",
            |id: String| {
                assert!(trigger_matches(
                    "step_complete",
                    &TriggerType::StepComplete(id)
                ));
            },
        );
    }

    #[test]
    fn trigger_matches_specific_step_complete() {
        run_proptest(
            ".{0,20}",
            |id: String| {
                let trigger = format!("step_complete:{}", id);
                assert!(trigger_matches(&trigger, &TriggerType::StepComplete(id.clone())));
                let other = format!("{}x", id);
                if other != id {
                    assert!(!trigger_matches(
                        &trigger,
                        &TriggerType::StepComplete(other)
                    ));
                }
            },
        );
    }

    #[test]
    fn trigger_matches_exact() {
        run_proptest(
            (arb_trigger_type(), ".{0,30}"),
            |(event, trigger): (TriggerType, String)| {
                match &event {
                    TriggerType::UserMessage => {
                        assert_eq!(
                            trigger_matches(&trigger, &event),
                            trigger == "user_message"
                        );
                    }
                    TriggerType::LlmComplete => {
                        assert_eq!(
                            trigger_matches(&trigger, &event),
                            trigger == "llm_complete"
                        );
                    }
                    TriggerType::SchedulerTick => {
                        assert_eq!(
                            trigger_matches(&trigger, &event),
                            trigger == "scheduler_tick"
                        );
                    }
                    TriggerType::StepComplete(id) => {
                        let expected = trigger == format!("step_complete:{}", id).as_str()
                            || trigger == "step_complete";
                        assert_eq!(trigger_matches(&trigger, &event), expected);
                    }
                }
            },
        );
    }

    #[test]
    fn is_enabled_when_no_field() {
        run_proptest(
            (".{0,20}", ".{0,30}", ".{0,30}"),
            |(step_id, step_type, trigger): (String, String, String)| {
                let step = StepDef {
                    id: step_id,
                    step_type,
                    trigger,
                    enabled_if: None,
                };
                assert!(is_enabled(&step, &SessionConfig::default()));
            },
        );
    }

    #[test]
    fn is_enabled_depends_on_key() {
        run_proptest(
            (".{0,20}", "[a-z._-]{1,20}", any::<bool>()),
            |(step_id, key, val): (String, String, bool)| {
                let step = StepDef {
                    id: step_id,
                    step_type: "llm".into(),
                    trigger: "user_message".into(),
                    enabled_if: Some(key.clone()),
                };
                let mut config = SessionConfig::default();
                config
                    .extra
                    .insert(key, serde_json::Value::Bool(val));
                assert_eq!(is_enabled(&step, &config), val);
            },
        );
    }

    #[test]
    fn build_rpc_params_always_returns_object() {
        run_proptest(
            (arb_namespace(), arb_pipeline_context()),
            |(namespace, ctx): (String, PipelineContext)| {
                let params = build_rpc_params(&namespace, &ctx);
                assert!(params.is_object());
            },
        );
    }

    #[test]
    fn build_rpc_params_llm_has_session_id() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let params = build_rpc_params("llm", &ctx);
            assert_eq!(params["session_id"], ctx.session_id);
        });
    }

    #[test]
    fn build_rpc_params_tts_has_text() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let params = build_rpc_params("tts", &ctx);
            assert!(params["text"].is_string());
        });
    }

    #[test]
    fn build_rpc_params_comfy_has_prompt() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let params = build_rpc_params("comfy", &ctx);
            assert!(params["prompt"].is_string());
        });
    }

    #[test]
    fn build_rpc_params_stt_has_session_id() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let params = build_rpc_params("stt", &ctx);
            assert_eq!(params["session_id"], ctx.session_id);
        });
    }

    #[test]
    fn build_rpc_params_fallback_has_session_id() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let params = build_rpc_params("unknown-namespace", &ctx);
            assert!(params["session_id"].is_string());
        });
    }
}
