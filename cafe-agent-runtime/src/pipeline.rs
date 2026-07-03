use crate::config::{resolve_session_config, SessionConfig};
use crate::tool_detector;
use crate::tool_executor;
use cafe_sdk::bus::BusClient;
use cafe_sdk::ToolCall;
use anyhow::Result;
use cafe_sdk::{
    keys, roles, Chunk, ContentType, JsonRpcRequest, JsonRpcResponse, SdkError, ServerMessage,
};
use std::time::Duration;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Step types
// ---------------------------------------------------------------------------

/// A single pipeline step parsed from the agent TOML `pipeline` array.
#[derive(Debug, Clone)]
pub enum PipelineStep {
    /// Executed in-process by cafe-agent-runtime (no external deps).
    BuiltIn(BuiltInStep),
    /// Dispatched via JSON-RPC to an external service binary.
    Rpc(String),
}

impl PipelineStep {
    pub fn from_name(name: &str) -> Self {
        match name {
            "role-annotator" => PipelineStep::BuiltIn(BuiltInStep::RoleAnnotator),
            "trust-filter" => PipelineStep::BuiltIn(BuiltInStep::TrustFilter),
            "tool-detector" => PipelineStep::BuiltIn(BuiltInStep::ToolDetector),
            "tool-executor" => PipelineStep::BuiltIn(BuiltInStep::ToolExecutor),
            other => PipelineStep::Rpc(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum BuiltInStep {
    RoleAnnotator,
    TrustFilter,
    ToolDetector,
    ToolExecutor,
}

// ---------------------------------------------------------------------------
// Pipeline executor
// ---------------------------------------------------------------------------

/// Holds the ordered list of pipeline steps and RPC timeout.
#[derive(Clone)]
pub struct PipelineExecutor {
    steps: Vec<PipelineStep>,
    rpc_timeout: Duration,
}

impl PipelineExecutor {
    pub fn from_step_names(names: &[String], rpc_timeout: Duration) -> Self {
        Self {
            steps: names.iter().map(|n| PipelineStep::from_name(n)).collect(),
            rpc_timeout,
        }
    }
}

// ---------------------------------------------------------------------------
// RPC dispatch (per-step)
// ---------------------------------------------------------------------------

/// Build the JSON-RPC params map from the assembled text and session config.
fn build_rpc_params(
    namespace: &str,
    text: &str,
    config: &SessionConfig,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "text": text,
    });

    // Merge extra config keys prefixed with the namespace (e.g. config.tts.*)
    // so the RPC handler gets the session-level settings.
    let prefix = format!("config.{}.", namespace);
    let mut extra = serde_json::Map::new();
    for (k, v) in &config.extra {
        if let Some(suffix) = k.strip_prefix(&prefix) {
            extra.insert(suffix.to_string(), v.clone());
        }
    }
    if !extra.is_empty() {
        params["config"] = serde_json::Value::Object(extra);
    }

    params
}

fn assemble_chunks(chunks: &[Chunk]) -> String {
    let mut text = String::new();
    for c in chunks {
        if c.content_type == ContentType::Text {
            if let Some(content) = &c.content {
                text.push_str(content);
                text.push('\n');
            }
        }
    }
    text.trim().to_string()
}

impl PipelineExecutor {
    /// Run the full pipeline for a session by dispatching each step that
    /// hasn't been handled yet. Called when a new assistant text chunk arrives.
    async fn run(
        &self,
        session_id: &str,
        client: &BusClient,
        assembled_text: &str,
        pending_tool_calls: &mut Option<Vec<ToolCall>>,
    ) -> Result<(), PipelineError> {
        let history = client.get_history(session_id).await?;
        let config = resolve_session_config(&history);

        for step in &self.steps {
            match step {
                PipelineStep::BuiltIn(BuiltInStep::RoleAnnotator) => {
                    // Roles are already annotated by the producer; no-op for now.
                }
                PipelineStep::BuiltIn(BuiltInStep::TrustFilter) => {
                    // Security trust-level filtering TBD; no-op for now.
                }
                PipelineStep::BuiltIn(BuiltInStep::ToolDetector) => {
                    let (_, calls) = tool_detector::detect(assembled_text);
                    if !calls.is_empty() {
                        info!(
                            "pipeline: detected {} tool call(s) in session {}",
                            calls.len(),
                            session_id
                        );
                        *pending_tool_calls = Some(calls);
                    }
                }
                PipelineStep::BuiltIn(BuiltInStep::ToolExecutor) => {
                    if let Some(calls) = pending_tool_calls.take() {
                        for call in &calls {
                            info!(
                                "pipeline: executing tool '{}' for session {}",
                                call.name, session_id
                            );
                            tool_executor::execute(call, session_id, client)
                                .await
                                .map_err(|e| PipelineError::Io(e.into()))?;
                        }
                    }
                }
                PipelineStep::Rpc(namespace) => {
                    if namespace != "tts" && namespace != "comfy" {
                        continue;
                    }
                    let params = build_rpc_params(namespace, assembled_text, &config);
                    let method = format!("{}.invoke", namespace);

                    let request = JsonRpcRequest::new(&method, params);
                    let call_id = request.id.clone();

                    info!(
                        "pipeline: dispatching RPC {method} call_id={call_id} session={session_id}"
                    );

                    // Drain history first, then subscribe for live response
                    client.get_history(session_id).await?;

                    let req_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                        .with_annotation(keys::JSONRPC_REQUEST, &request);
                    client.publish(session_id, req_chunk).await?;

                    let mut rx = client.subscribe(session_id).await?;

                    let response: JsonRpcResponse =
                        tokio::time::timeout(self.rpc_timeout, async {
                            loop {
                                match rx.recv().await {
                                    Some(ServerMessage::Chunk { chunk, .. }) => {
                                        if chunk.is_rpc_response_for(&call_id) {
                                            return chunk.as_rpc_response().ok_or_else(|| {
                                                anyhow::anyhow!(
                                                    "failed to deserialize RPC response"
                                                )
                                            });
                                        }
                                    }
                                    Some(_) => continue,
                                    None => {
                                        anyhow::bail!("bus disconnected while waiting for RPC response");
                                    }
                                }
                            }
                        })
                        .await
                        .map_err(|_| PipelineError::Timeout {
                            step: namespace.to_string(),
                            call_id: call_id.clone(),
                        })?
                        .map_err(PipelineError::Io)?;

                    if response.is_ok() {
                        info!(
                            "pipeline: RPC {method} succeeded call_id={call_id} session={session_id}"
                        );
                    } else {
                        let err = response.error.unwrap();
                        return Err(PipelineError::RpcError {
                            step: namespace.to_string(),
                            code: err.code,
                            message: err.message,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session listener
// ---------------------------------------------------------------------------

/// Subscribe to a session and run the pipeline whenever the LLM completes
/// a response (i.e. `chat.stream_complete` is observed after a user turn).
pub async fn run_session_pipeline(
    session_id: String,
    socket_path: String,
    executor: PipelineExecutor,
) -> Result<()> {
    let client = BusClient::new(&socket_path);
    let mut rx = client.subscribe(&session_id).await?;

    let mut history: Vec<Chunk> = Vec::new();
    let mut history_complete = false;
    let mut _last_user_idx: Option<usize> = None;
    let mut llm_active = false;

    while let Some(msg) = rx.recv().await {
        match msg {
            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                for (i, c) in history.iter().enumerate() {
                    if c.content_type == ContentType::Text && c.role() == Some(roles::USER) {
                        _last_user_idx = Some(i);
                    }
                }
            }
            ServerMessage::Chunk { chunk, .. } => {
                if !history_complete {
                    history.push(chunk);
                    continue;
                }

                // Track chat turns
                if chunk.role() == Some(roles::USER)
                    && chunk.content_type == ContentType::Text
                {
                    llm_active = true;
                }

                // When the LLM finishes a response turn, run the pipeline
                if llm_active
                    && chunk
                        .get_annotation::<bool>("chat.stream_complete")
                        .unwrap_or(false)
                {
                    llm_active = false;

                    // Re-fetch assembled text for this turn
                    let fresh_history = client.get_history(&session_id).await?;
                    let text = assemble_chunks(&fresh_history);

                    debug!(
                        "pipeline: running pipeline for session {} ({} chars)",
                        session_id,
                        text.len()
                    );

                    let mut pending_tool_calls: Option<Vec<ToolCall>> = None;
                    if let Err(e) = executor
                        .run(&session_id, &client, &text, &mut pending_tool_calls)
                        .await
                    {
                        warn!("pipeline: step failed for session {}: {}", session_id, e);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("SDK error: {0}")]
    Sdk(#[from] SdkError),

    #[error("I/O error: {0}")]
    Io(#[source] anyhow::Error),

    #[error("RPC step '{step}' timed out waiting for call_id={call_id}")]
    Timeout {
        step: String,
        call_id: String,
    },

    #[error("RPC step '{step}' returned error {code}: {message}")]
    RpcError {
        step: String,
        code: i32,
        message: String,
    },
}
