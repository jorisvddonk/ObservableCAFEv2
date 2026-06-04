//! Pipeline executor for cafe-agent-runtime.
//!
//! The executor walks the pipeline steps defined in the agent TOML. Built-in
//! steps run in-process; RPC steps publish a JSON-RPC request as a null chunk
//! on the session bus and await a matching response.

use crate::config::{resolve_session_config, SessionConfig};
use anyhow::Result;
use cafe_types::{
    keys, roles, Chunk, ClientMessage, ContentType, JsonRpcRequest, JsonRpcResponse,
    ServerMessage,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
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
    Rpc(String), // namespace, e.g. "tts", "stt"
}

/// Built-in step variants.
#[derive(Debug, Clone)]
pub enum BuiltInStep {
    /// Annotates chunks with `chat.role` if not already set.
    RoleAnnotator,
    /// Filters chunks based on `security.trust-level`.
    TrustFilter,
}

impl PipelineStep {
    /// Parse a step name from agent TOML into a `PipelineStep`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "role-annotator" => PipelineStep::BuiltIn(BuiltInStep::RoleAnnotator),
            "trust-filter" => PipelineStep::BuiltIn(BuiltInStep::TrustFilter),
            other => PipelineStep::Rpc(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("RPC step '{step}' timed out waiting for response to call {call_id}")]
    Timeout { step: String, call_id: String },

    #[error("RPC step '{step}' returned error: [{code}] {message}")]
    RpcError {
        step: String,
        code: i32,
        message: String,
    },

    #[error("pipeline I/O error: {0}")]
    Io(#[from] anyhow::Error),
}

// ---------------------------------------------------------------------------
// Pipeline executor
// ---------------------------------------------------------------------------

/// Runs one pipeline for a session.
pub struct PipelineExecutor {
    pub steps: Vec<PipelineStep>,
    /// How long to wait for an RPC response before timing out.
    pub rpc_timeout: Duration,
}

impl Default for PipelineExecutor {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            rpc_timeout: Duration::from_secs(30),
        }
    }
}

impl PipelineExecutor {
    /// Build an executor from a list of step names (as stored in `AgentDefinition.pipeline`).
    pub fn from_step_names(names: &[String], rpc_timeout: Duration) -> Self {
        Self {
            steps: names.iter().map(|n| PipelineStep::from_name(n)).collect(),
            rpc_timeout,
        }
    }

    /// Walk the pipeline steps after the LLM has completed its response for
    /// the given session. The `history` slice must include all chunks up to
    /// and including the `chat.stream_complete` marker.
    ///
    /// `socket_path` is used to open a fresh connection for publishing and
    /// awaiting chunks on the bus.
    pub async fn run_post_llm(
        &self,
        session_id: &str,
        history: &[Chunk],
        socket_path: &str,
    ) -> Result<(), PipelineError> {
        // Collect the assistant text assembled from streaming chunks
        let assembled_text = assemble_assistant_text(history);

        if assembled_text.is_empty() {
            debug!(
                "pipeline: no assistant text in session {}, skipping post-LLM steps",
                session_id
            );
            return Ok(());
        }

        let config = resolve_session_config(history);

        for step in &self.steps {
            match step {
                PipelineStep::BuiltIn(_) => {
                    // Built-in steps in the post-LLM phase are no-ops for now;
                    // they run implicitly on user input before the LLM fires.
                }

                PipelineStep::Rpc(namespace) => {
                    // Only run post-LLM RPC steps; (stt, llm) are handled elsewhere.
                    if namespace != "tts" && namespace != "comfy" {
                        continue;
                    }

                    self.dispatch_rpc(
                        session_id,
                        namespace,
                        &assembled_text,
                        &config,
                        socket_path,
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    /// Publish a JSON-RPC request for `namespace` and wait for the matching
    /// response on the session stream.
    async fn dispatch_rpc(
        &self,
        session_id: &str,
        namespace: &str,
        assembled_text: &str,
        config: &SessionConfig,
        socket_path: &str,
    ) -> Result<(), PipelineError> {
        let params = build_rpc_params(namespace, assembled_text, config);
        let method = format!("{}.invoke", namespace);

        let request = JsonRpcRequest::new(&method, params);
        let call_id = request.id.clone();

        info!(
            "pipeline: dispatching RPC {method} call_id={call_id} session={session_id}"
        );

        // Open a dedicated connection for this dispatch
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| PipelineError::Io(e.into()))?;
        let (reader, mut writer) = stream.into_split();

        // Subscribe to the session so we can see response chunks
        let sub_msg = serde_json::to_string(&ClientMessage::Subscribe {
            session_id: session_id.to_string(),
        })
        .unwrap()
            + "\n";
        writer
            .write_all(sub_msg.as_bytes())
            .await
            .map_err(|e| PipelineError::Io(e.into()))?;

        // Drain history replay (HistoryComplete marker) before publishing request
        let mut lines = BufReader::new(reader).lines();
        loop {
            let line = lines
                .next_line()
                .await
                .map_err(|e| PipelineError::Io(e.into()))?
                .ok_or_else(|| PipelineError::Io(anyhow::anyhow!("bus disconnected")))?;

            let msg: ServerMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };

            if matches!(msg, ServerMessage::HistoryComplete { .. }) {
                break;
            }
        }

        // Publish the RPC request as a null chunk
        let req_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
            .with_annotation(keys::JSONRPC_REQUEST, &request);
        let pub_msg = ClientMessage::Publish {
            session_id: session_id.to_string(),
            chunk: req_chunk,
        };
        let mut pub_json = serde_json::to_string(&pub_msg).unwrap();
        pub_json.push('\n');
        writer
            .write_all(pub_json.as_bytes())
            .await
            .map_err(|e| PipelineError::Io(e.into()))?;

        // Await matching response with timeout
        let timeout = self.rpc_timeout;
        let response: JsonRpcResponse = tokio::time::timeout(timeout, async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .map_err(|e: std::io::Error| anyhow::anyhow!(e))?
                    .ok_or_else(|| anyhow::anyhow!("bus disconnected while waiting for RPC response"))?;

                let msg: ServerMessage = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if let ServerMessage::Chunk { chunk, .. } = msg {
                    if chunk.is_rpc_response_for(&call_id) {
                        return chunk
                            .as_rpc_response()
                            .ok_or_else(|| anyhow::anyhow!("failed to deserialize RPC response"));
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

        if !response.is_ok() {
            let err = response.error.unwrap();
            return Err(PipelineError::RpcError {
                step: namespace.to_string(),
                code: err.code,
                message: err.message,
            });
        }

        info!(
            "pipeline: RPC {method} succeeded call_id={call_id} session={session_id}"
        );

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
    let stream = UnixStream::connect(&socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    let sub_msg = serde_json::to_string(&ClientMessage::Subscribe {
        session_id: session_id.clone(),
    })? + "\n";
    writer.write_all(sub_msg.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    let mut history: Vec<Chunk> = Vec::new();
    let mut history_complete = false;
    let mut last_user_idx: Option<usize> = None;
    let mut llm_active = false;

    while let Some(line) = lines.next_line().await? {
        let msg: ServerMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("pipeline: invalid bus message: {}", e);
                continue;
            }
        };

        match msg {
            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                // Track the last user chunk in replayed history
                for (i, c) in history.iter().enumerate() {
                    if c.content_type == ContentType::Text && c.role() == Some(roles::USER) {
                        last_user_idx = Some(i);
                    }
                }
                debug!(
                    "pipeline: history replay complete for {} ({} chunks)",
                    session_id,
                    history.len()
                );
            }

            ServerMessage::Chunk { session_id: sid, chunk } if sid == session_id => {
                // Track user turns
                if history_complete
                    && chunk.content_type == ContentType::Text
                    && chunk.role() == Some(roles::USER)
                {
                    last_user_idx = Some(history.len());
                    llm_active = true;
                }

                // Detect LLM stream completion
                let is_stream_complete = chunk
                    .get_annotation::<bool>(keys::CHAT_STREAM_COMPLETE)
                    .unwrap_or(false);

                history.push(chunk);

                if history_complete && llm_active && is_stream_complete {
                    llm_active = false;

                    // Only run pipeline if there was a user turn before this completion
                    if last_user_idx.is_some() {
                        info!(
                            "pipeline: LLM complete for session {}, running post-LLM steps",
                            session_id
                        );
                        let history_snapshot = history.clone();
                        let sid = session_id.clone();
                        let sp = socket_path.clone();
                        let exec = PipelineExecutor {
                            steps: executor.steps.clone(),
                            rpc_timeout: executor.rpc_timeout,
                        };
                        tokio::spawn(async move {
                            if let Err(e) = exec.run_post_llm(&sid, &history_snapshot, &sp).await {
                                warn!("pipeline: post-LLM error for session {}: {}", sid, e);
                            }
                        });
                    }
                }
            }

            _ => {}
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect the full assistant response text from streaming history chunks.
///
/// Scans from the last user message forward and concatenates all text chunks
/// with `chat.role: assistant` and `chat.is_streaming: true`.
pub fn assemble_assistant_text(history: &[Chunk]) -> String {
    // Find the index of the last user chunk
    let start = history
        .iter()
        .rposition(|c| c.content_type == ContentType::Text && c.role() == Some(roles::USER))
        .map(|i| i + 1)
        .unwrap_or(0);

    history[start..]
        .iter()
        .filter(|c| {
            c.content_type == ContentType::Text
                && c.role() == Some(roles::ASSISTANT)
                && c.get_annotation::<bool>(keys::CHAT_IS_STREAMING)
                    .unwrap_or(false)
        })
        .filter_map(|c| c.content.as_deref())
        .collect::<Vec<_>>()
        .join("")
}

/// Build JSON-RPC params for a given namespace.
fn build_rpc_params(
    namespace: &str,
    assembled_text: &str,
    config: &SessionConfig,
) -> serde_json::Value {
    match namespace {
        "tts" => serde_json::json!({
            "text": assembled_text,
            "profile": config.tts_profile.as_deref().unwrap_or("default"),
            "engine": config.tts_engine,
        }),
        "comfy" => serde_json::json!({
            "text": assembled_text,
            "workflow_path": config.comfy_workflow_path,
            "workflow_input_node": config.comfy_workflow_input_node,
            "endpoint": config.comfy_endpoint,
        }),
        "stt" => serde_json::json!({
            "base_url": config.stt_base_url,
        }),
        _ => serde_json::json!({}),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_types::Chunk;

    fn user_chunk(text: &str) -> Chunk {
        Chunk::new_text(text, "user").with_annotation(keys::CHAT_ROLE, roles::USER)
    }

    fn assistant_streaming_chunk(text: &str) -> Chunk {
        Chunk::new_text(text, "com.nominal.cafe-llm")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
            .with_annotation(keys::CHAT_IS_STREAMING, true)
    }

    fn stream_complete_chunk() -> Chunk {
        Chunk::new_null("com.nominal.cafe-llm")
            .with_annotation(keys::CHAT_STREAM_COMPLETE, true)
    }

    #[test]
    fn assemble_assistant_text_collects_streaming_chunks() {
        let history = vec![
            user_chunk("hello"),
            assistant_streaming_chunk("Hi "),
            assistant_streaming_chunk("there!"),
            stream_complete_chunk(),
        ];
        assert_eq!(assemble_assistant_text(&history), "Hi there!");
    }

    #[test]
    fn assemble_assistant_text_empty_with_no_streaming_chunks() {
        let history = vec![user_chunk("hello"), stream_complete_chunk()];
        assert_eq!(assemble_assistant_text(&history), "");
    }

    #[test]
    fn assemble_assistant_text_only_uses_latest_turn() {
        // Two user turns; only second turn's assistant text should be returned
        let history = vec![
            user_chunk("turn 1"),
            assistant_streaming_chunk("Response 1"),
            stream_complete_chunk(),
            user_chunk("turn 2"),
            assistant_streaming_chunk("Response 2"),
            stream_complete_chunk(),
        ];
        assert_eq!(assemble_assistant_text(&history), "Response 2");
    }

    #[test]
    fn pipeline_step_from_name() {
        assert!(matches!(
            PipelineStep::from_name("role-annotator"),
            PipelineStep::BuiltIn(BuiltInStep::RoleAnnotator)
        ));
        assert!(matches!(
            PipelineStep::from_name("trust-filter"),
            PipelineStep::BuiltIn(BuiltInStep::TrustFilter)
        ));
        assert!(matches!(
            PipelineStep::from_name("tts"),
            PipelineStep::Rpc(ref s) if s == "tts"
        ));
        assert!(matches!(
            PipelineStep::from_name("stt"),
            PipelineStep::Rpc(ref s) if s == "stt"
        ));
        assert!(matches!(
            PipelineStep::from_name("llm"),
            PipelineStep::Rpc(ref s) if s == "llm"
        ));
    }

    #[test]
    fn executor_from_step_names() {
        let names = vec![
            "trust-filter".into(),
            "llm".into(),
            "tts".into(),
        ];
        let exec = PipelineExecutor::from_step_names(&names, Duration::from_secs(30));
        assert_eq!(exec.steps.len(), 3);
        assert_eq!(exec.rpc_timeout, Duration::from_secs(30));
    }

    #[test]
    fn build_rpc_params_tts() {
        let config = SessionConfig {
            tts_profile: Some("Volition".into()),
            tts_engine: Some("qwen".into()),
            ..Default::default()
        };
        let params = build_rpc_params("tts", "Hello world", &config);
        assert_eq!(params["text"], "Hello world");
        assert_eq!(params["profile"], "Volition");
        assert_eq!(params["engine"], "qwen");
    }

    #[test]
    fn build_rpc_params_tts_defaults() {
        let config = SessionConfig::default();
        let params = build_rpc_params("tts", "Hi", &config);
        assert_eq!(params["profile"], "default");
    }

    #[test]
    fn build_rpc_params_comfy() {
        let config = SessionConfig {
            comfy_workflow_path: Some("my_workflow.json".into()),
            comfy_workflow_input_node: Some("3".into()),
            comfy_endpoint: Some("http://localhost:8188".into()),
            ..Default::default()
        };
        let params = build_rpc_params("comfy", "a cat in space", &config);
        assert_eq!(params["text"], "a cat in space");
        assert_eq!(params["workflow_path"], "my_workflow.json");
        assert_eq!(params["workflow_input_node"], "3");
        assert_eq!(params["endpoint"], "http://localhost:8188");
    }

    #[test]
    fn build_rpc_params_comfy_defaults() {
        let config = SessionConfig::default();
        let params = build_rpc_params("comfy", "a dog", &config);
        assert_eq!(params["text"], "a dog");
        assert!(params["workflow_path"].is_null());
        assert!(params["workflow_input_node"].is_null());
    }
}
