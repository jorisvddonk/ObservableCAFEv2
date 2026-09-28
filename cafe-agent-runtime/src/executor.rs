use crate::config::{self, SessionConfig};
use crate::schema_registry::SchemaRegistry;
use crate::tool_detector;
use crate::tool_executor;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{
    keys, Chunk, EvaluatorSchema, JsonRpcRequest, JsonRpcResponse, SdkError, ServerMessage, StepDef,
};
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
/// Schemas come from the live [`SchemaRegistry`] (ADR-121): param building
/// and config resolution consult them, so adding an evaluator needs no
/// changes here.
#[derive(Clone)]
pub struct PipelineExecutor {
    steps: Vec<StepDef>,
    rpc_timeout: Duration,
    max_depth: u32,
    schemas: SchemaRegistry,
}

impl PipelineExecutor {
    pub fn new(
        steps: Vec<StepDef>,
        rpc_timeout: Duration,
        max_depth: u32,
        schemas: SchemaRegistry,
    ) -> Self {
        Self {
            steps,
            rpc_timeout,
            max_depth,
            schemas,
        }
    }

    /// Resolve the session config against currently known evaluator schemas.
    pub async fn resolve_config(&self, history: &[Chunk]) -> SessionConfig {
        let schemas = self.schemas.snapshot().await;
        config::resolve_session_config(history, &schemas)
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
            .values
            .get(key.as_str())
            .or_else(|| config.extra.get(key.as_str()))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

/// Build RPC params for a given namespace and pipeline context, driven by the
/// evaluator's announced schema (ADR-121) instead of per-evaluator code.
///
/// Rules, in order, for each property declared in `rpc_params_schema`:
/// - `session_id` → the session id (always sent, as before).
/// - `text` / `prompt` → the assembled LLM text, else the user text, else `""`.
///   (`prompt` is the legacy LLM-output alias cafe-comfy accepts.)
/// - anything else → the session config value at the key named by the
///   property's `x-config-key` extension, or `config.{namespace}.{name}` when
///   absent; skipped when unset.
///
/// With no schema (unknown or not-yet-announced evaluator), the historical
/// default applies: `session_id` plus `text` when available. All known
/// handlers read params with `as_str().unwrap_or(...)`, so sending `""`
/// instead of omitting is equivalent — and `session_id` extras are ignored.
fn build_rpc_params(
    namespace: &str,
    ctx: &PipelineContext,
    schema: Option<&EvaluatorSchema>,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "session_id": ctx.session_id,
    });

    let text = || {
        ctx.assembled_llm_text
            .as_deref()
            .or_else(|| ctx.user_text.as_deref())
            .unwrap_or("")
    };

    match schema.and_then(|s| s.rpc_params_schema.get("properties")) {
        Some(props) if props.is_object() => {
            // serde_json::Map iterates in sorted key order — deterministic.
            for (name, spec) in props.as_object().expect("checked is_object") {
                match name.as_str() {
                    "session_id" => {}
                    "text" | "prompt" => {
                        params[name] = serde_json::Value::String(text().to_string());
                    }
                    other => {
                        let config_key = spec
                            .get("x-config-key")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| format!("config.{namespace}.{other}"));
                        if let Some(value) = ctx.config.values.get(&config_key) {
                            params[other] = value.clone();
                        }
                    }
                }
            }
        }
        _ => {
            if let Some(t) = ctx
                .assembled_llm_text
                .as_deref()
                .or_else(|| ctx.user_text.as_deref())
            {
                params["text"] = serde_json::Value::String(t.to_string());
            }
        }
    }

    params
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
        let schema = self.schemas.get(namespace).await;
        let params = build_rpc_params(namespace, context, schema.as_ref());
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

    #[test]
    fn pipeline_error_display_never_panics() {
        // Generate arbitrary data for each PipelineError variant
        run_proptest(
            (".{0,30}", ".*"),
            |(step_id, call_id): (String, String)| {
                let err = PipelineError::Timeout { step_id, call_id };
                let _ = format!("{}", err);
            },
        );
        run_proptest(
            (".{0,30}", any::<i32>(), ".*"),
            |(step_id, code, message): (String, i32, String)| {
                let err = PipelineError::RpcError { step_id, code, message };
                let _ = format!("{}", err);
            },
        );
        run_proptest(
            (".{0,30}", ".*"),
            |(step_id, msg): (String, String)| {
                let err = PipelineError::Bus {
                    step_id,
                    source: anyhow::anyhow!("{}", msg),
                };
                let _ = format!("{}", err);
            },
        );
        run_proptest(
            ".*",
            |msg: String| {
                let err = PipelineError::Sdk(SdkError::BusError {
                    message: msg.clone(),
                    code: Some(msg),
                });
                let _ = format!("{}", err);
            },
        );
        run_proptest(
            ".*",
            |msg: String| {
                let err = PipelineError::Io(anyhow::anyhow!("{}", msg));
                let _ = format!("{}", err);
            },
        );
    }

    #[test]
    fn pipeline_error_timeout_display_has_step_id_and_call_id() {
        run_proptest(
            (".{0,30}", ".*"),
            |(step_id, call_id): (String, String)| {
                let err = PipelineError::Timeout {
                    step_id: step_id.clone(),
                    call_id: call_id.clone(),
                };
                let s = format!("{}", err);
                assert!(s.contains(&step_id), "Display must contain step_id");
                assert!(s.contains(&call_id), "Display must contain call_id");
            },
        );
    }

    #[test]
    fn pipeline_error_rpc_error_display_has_step_id_and_message() {
        run_proptest(
            (".{0,30}", any::<i32>(), ".*"),
            |(step_id, code, message): (String, i32, String)| {
                let err = PipelineError::RpcError {
                    step_id: step_id.clone(),
                    code,
                    message: message.clone(),
                };
                let s = format!("{}", err);
                assert!(s.contains(&step_id), "Display must contain step_id");
            },
        );
    }

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
        prop::collection::hash_map("[a-z._-]{1,30}", arb_annotation_value(), 0..5).prop_map(
            |entries| {
                let extra: std::collections::HashMap<String, serde_json::Value> =
                    entries.clone().into_iter().collect();
                SessionConfig {
                    values: entries,
                    extra,
                }
            },
        )
    }

    /// A schema with no declared params (unknown evaluator / not yet announced).
    fn empty_params_schema() -> EvaluatorSchema {
        EvaluatorSchema {
            name: "unknown".into(),
            description: "no params".into(),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    /// A schema declaring `session_id` plus the given property names, all
    /// typed as strings with no per-property special casing.
    fn params_schema(namespace: &str, props: &[&str]) -> EvaluatorSchema {
        let mut properties = serde_json::Map::new();
        properties.insert(
            "session_id".into(),
            serde_json::json!({"type": "string"}),
        );
        for p in props {
            properties.insert((*p).into(), serde_json::json!({"type": "string"}));
        }
        EvaluatorSchema {
            name: namespace.into(),
            description: format!("{namespace} params"),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({
                "type": "object",
                "properties": serde_json::Value::Object(properties),
            }),
        }
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
                let params = build_rpc_params(&namespace, &ctx, None);
                assert!(params.is_object());
            },
        );
    }

    #[test]
    fn build_rpc_params_llm_has_session_id() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let schema = params_schema("llm", &[]);
            let params = build_rpc_params("llm", &ctx, Some(&schema));
            assert_eq!(params["session_id"], ctx.session_id);
        });
    }

    #[test]
    fn build_rpc_params_tts_has_text_from_schema() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            let schema = params_schema("tts", &["text", "profile", "engine"]);
            let params = build_rpc_params("tts", &ctx, Some(&schema));
            // `text` is always populated (assembled LLM text, else user text,
            // else the empty string) so downstream consumers never see null.
            assert!(params["text"].is_string());
            assert_eq!(params["session_id"], ctx.session_id);
        });
    }

    #[test]
    fn build_rpc_params_comfy_has_prompt_from_schema() {
        run_proptest(arb_pipeline_context(), |ctx: PipelineContext| {
            // cafe-comfy declares `prompt` (legacy alias for assembled text).
            let schema = params_schema("comfy", &["prompt", "workflow_path", "input_node"]);
            let params = build_rpc_params("comfy", &ctx, Some(&schema));
            assert!(params["prompt"].is_string());
            assert_eq!(params["session_id"], ctx.session_id);
        });
    }

    #[test]
    fn build_rpc_params_copies_declared_config_values() {
        let mut ctx = PipelineContext {
            session_id: "s1".into(),
            config: SessionConfig::default(),
            assembled_llm_text: Some("hello".into()),
            user_text: None,
            depth: 0,
        };
        ctx.config.values.insert(
            "config.tts.profile".into(),
            serde_json::json!("narrator"),
        );
        let schema = params_schema("tts", &["text", "profile"]);
        let params = build_rpc_params("tts", &ctx, Some(&schema));
        assert_eq!(params["profile"], "narrator");
        assert_eq!(params["text"], "hello");
    }

    #[test]
    fn build_rpc_params_omits_unset_config_values() {
        let ctx = PipelineContext {
            session_id: "s1".into(),
            config: SessionConfig::default(),
            assembled_llm_text: Some("hi".into()),
            user_text: None,
            depth: 0,
        };
        let schema = params_schema("tts", &["text", "profile"]);
        let params = build_rpc_params("tts", &ctx, Some(&schema));
        // Unset config keys are omitted, never sent as null.
        assert!(params.get("profile").is_none());
    }

    #[test]
    fn build_rpc_params_honours_x_config_key() {
        let mut ctx = PipelineContext {
            session_id: "s1".into(),
            config: SessionConfig::default(),
            assembled_llm_text: Some("hi".into()),
            user_text: None,
            depth: 0,
        };
        // comfy's `input_node` param maps to `config.comfy.workflow_input_node`.
        ctx.config.values.insert(
            "config.comfy.workflow_input_node".into(),
            serde_json::json!("7"),
        );
        let schema = EvaluatorSchema {
            name: "comfy".into(),
            description: "comfy".into(),
            config_schema: serde_json::json!({"type": "object", "properties": {}}),
            rpc_params_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "input_node": {
                        "type": "string",
                        "x-config-key": "config.comfy.workflow_input_node"
                    }
                }
            }),
        };
        let params = build_rpc_params("comfy", &ctx, Some(&schema));
        assert_eq!(params["input_node"], "7");
    }

    #[test]
    fn build_rpc_params_falls_back_to_user_text() {
        let ctx = PipelineContext {
            session_id: "s1".into(),
            config: SessionConfig::default(),
            assembled_llm_text: None,
            user_text: Some("from user".into()),
            depth: 0,
        };
        let schema = params_schema("comfy", &["prompt"]);
        let params = build_rpc_params("comfy", &ctx, Some(&schema));
        assert_eq!(params["prompt"], "from user");
    }

    #[test]
    fn build_rpc_params_without_schema_uses_legacy_default() {
        let ctx = PipelineContext {
            session_id: "s1".into(),
            config: SessionConfig::default(),
            assembled_llm_text: Some("legacy".into()),
            user_text: None,
            depth: 0,
        };
        let params = build_rpc_params("whatever", &ctx, None);
        assert_eq!(params["session_id"], "s1");
        assert_eq!(params["text"], "legacy");
    }

    #[test]
    fn build_rpc_params_empty_schema_omits_text() {
        let ctx = PipelineContext {
            session_id: "s1".into(),
            config: SessionConfig::default(),
            assembled_llm_text: Some("ignored".into()),
            user_text: None,
            depth: 0,
        };
        let schema = empty_params_schema();
        let params = build_rpc_params("stt", &ctx, Some(&schema));
        assert_eq!(params["session_id"], "s1");
        assert!(params.get("text").is_none());
    }
}
