use crate::comfyui::ComfyUIClient;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{
    keys, roles, rpc_errors, Chunk, EvaluatorSchema, JsonRpcRequest, JsonRpcResponse, ServerMessage,
};
use std::sync::Arc;
use tracing::{error, info, warn};

pub async fn run_with_reconnect(
    socket_path: String,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) {
    cafe_sdk::bus::run_with_reconnect("cafe-comfy", move || {
        let socket = socket_path.clone();
        let comfy = comfy.clone();
        let wf = workflow.clone();
        let inp = input_node.clone();
        async move { subscribe_sessions(&socket, comfy, &wf, &inp).await }
    })
    .await;
}

async fn subscribe_sessions(
    socket_path: &str,
    comfy: Arc<ComfyUIClient>,
    workflow: &serde_json::Value,
    input_node: &str,
) -> anyhow::Result<()> {
    info!("cafe-comfy: starting (subscribe-all mode) on {}", socket_path);

    let client = BusClient::unix(socket_path);

    let schema = EvaluatorSchema {
        name: "comfy".into(),
        description: "Image generation evaluator — runs a ComfyUI workflow and produces an image".into(),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "config.comfy.workflow_path": { "type": "string", "description": "Path to the ComfyUI workflow JSON" },
                "config.comfy.workflow_input_node": { "type": "string", "description": "Input node ID in the workflow" },
                "config.comfy.endpoint": { "type": "string", "description": "ComfyUI API endpoint" }
            }
        }),
        rpc_params_schema: serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": { "type": "string", "description": "Image generation prompt (alias: prompt)" },
                "prompt": { "type": "string", "description": "Legacy alias for text" },
                "workflow_path": { "type": "string", "description": "Override workflow file path (loaded per call)" },
                "input_node": { "type": "string", "description": "Override input node ID" },
                "endpoint": { "type": "string", "description": "Override ComfyUI base URL for this call" }
            }
        }),
    };
    if let Err(e) = cafe_sdk::schema::announce_schema(&client, schema).await {
        warn!("cafe-comfy: failed to announce schema: {}", e);
    }

    let mut rx = client.subscribe_all().await?;

    let wf = workflow.to_owned();
    let inp = input_node.to_string();

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let client = client.clone();
            let vb = comfy.clone();
            let wf = wf.clone();
            let inp = inp.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session_handler(session_id, client, vb, wf, inp).await {
                    warn!("cafe-comfy: session handler error: {}", e);
                }
            });
        }
    }

    Ok(())
}

async fn run_session_handler(
    session_id: String,
    client: BusClient,
    comfy: Arc<ComfyUIClient>,
    workflow: serde_json::Value,
    input_node: String,
) -> anyhow::Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    // Gate RPC dispatch on history replay completion (ADR-123).
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

        if !history_complete {
            continue;
        }

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("comfy.") { continue; }

        info!(
            "cafe-comfy: handling RPC request id={} method={} session={}",
            request.id, request.method, session_id
        );

        let call_id = request.id.clone();
        let result = handle_comfy_request(&comfy, &request, &workflow, &input_node, &client, &session_id).await;

        let response = match result {
            Ok(image_chunk_id) => JsonRpcResponse::ok(
                &call_id,
                serde_json::json!({ "chunk_id": image_chunk_id }),
            ),
            Err(e) => {
                error!("cafe-comfy: comfy error for call {}: {}", call_id, e);
                JsonRpcResponse::err(&call_id, rpc_errors::UPSTREAM_ERROR, e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-comfy")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response);
        let _ = client.publish(&session_id, resp_chunk).await;
    }
    Ok(())
}

async fn handle_comfy_request(
    comfy: &ComfyUIClient,
    request: &JsonRpcRequest,
    default_workflow: &serde_json::Value,
    default_input_node: &str,
    client: &BusClient,
    session_id: &str,
) -> anyhow::Result<String> {
    let params = &request.params;
    let text = resolve_text(params)?;
    let workflow = resolve_workflow(default_workflow, params)?;
    let input_node = resolve_input_node(default_input_node, params);

    // Per-call endpoint override gets a throwaway client; otherwise use the
    // shared one (configured from COMFY_URL at startup).
    let image_bytes = match params["endpoint"].as_str().filter(|s| !s.is_empty()) {
        Some(url) => {
            info!("cafe-comfy: using endpoint override {url}");
            ComfyUIClient::new(url).generate(&workflow, text, input_node).await?
        }
        None => comfy.generate(&workflow, text, input_node).await?,
    };

    let image_chunk = Chunk::new_binary(image_bytes, "image/png", "com.nominal.cafe-comfy")
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);

    let image_chunk_id = image_chunk.id.clone();
    let _ = client.publish(session_id, image_chunk).await;

    info!("cafe-comfy: published image chunk {} for session {}", image_chunk_id, session_id);
    Ok(image_chunk_id)
}

/// The prompt to render. `text` is canonical; `prompt` is accepted as an alias
/// for the legacy agent pipeline, which sent the LLM output as `prompt`.
fn resolve_text(params: &serde_json::Value) -> anyhow::Result<&str> {
    let text = params["text"]
        .as_str()
        .or_else(|| params["prompt"].as_str())
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("comfy.invoke: text param is empty");
    }
    Ok(text)
}

/// Workflow for this call: an inline `workflow_path` override loads that file,
/// otherwise the service default (loaded from `COMFY_WORKFLOW_PATH`) is used.
fn resolve_workflow(
    default_workflow: &serde_json::Value,
    params: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    match params["workflow_path"].as_str().filter(|s| !s.is_empty()) {
        Some(path) => {
            let body = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("reading workflow override '{path}': {e}"))?;
            serde_json::from_str(&body)
                .map_err(|e| anyhow::anyhow!("parsing workflow override '{path}': {e}"))
        }
        None => Ok(default_workflow.clone()),
    }
}

/// Input node for this call: `input_node` override, else the service default.
fn resolve_input_node<'a>(default_input_node: &'a str, params: &'a serde_json::Value) -> &'a str {
    params["input_node"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(default_input_node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: serde_json::Value) -> serde_json::Value {
        v
    }

    #[test]
    fn text_prefers_text_and_accepts_prompt_alias() {
        assert_eq!(resolve_text(&p(serde_json::json!({"text": "a"}))).unwrap(), "a");
        assert_eq!(resolve_text(&p(serde_json::json!({"prompt": "b"}))).unwrap(), "b");
        assert_eq!(
            resolve_text(&p(serde_json::json!({"text": "a", "prompt": "b"}))).unwrap(),
            "a"
        );
        assert!(resolve_text(&p(serde_json::json!({}))).is_err());
        assert!(resolve_text(&p(serde_json::json!({"text": ""}))).is_err());
    }

    #[test]
    fn workflow_defaults_to_service_value() {
        let default = serde_json::json!({"1": {"class_type": "X"}});
        let got = resolve_workflow(&default, &p(serde_json::json!({}))).unwrap();
        assert_eq!(got, default);
    }

    #[test]
    fn workflow_override_loads_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("wf.json");
        std::fs::write(&path, r#"{"2":{"class_type":"Y"}}"#).unwrap();
        let default = serde_json::json!({});
        let got = resolve_workflow(
            &default,
            &p(serde_json::json!({"workflow_path": path.to_str().unwrap()})),
        )
        .unwrap();
        assert_eq!(got["2"]["class_type"], "Y");
    }

    #[test]
    fn workflow_override_missing_file_errors() {
        let err = resolve_workflow(
            &serde_json::json!({}),
            &p(serde_json::json!({"workflow_path": "/nonexistent/wf.json"})),
        )
        .unwrap_err();
        assert!(err.to_string().contains("reading workflow override"));
    }

    #[test]
    fn input_node_override_wins() {
        let with_override = p(serde_json::json!({"input_node": "9"}));
        assert_eq!(resolve_input_node("6", &with_override), "9");
        assert_eq!(resolve_input_node("6", &p(serde_json::json!({}))), "6");
        assert_eq!(resolve_input_node("6", &p(serde_json::json!({"input_node": ""}))), "6");
    }
}
