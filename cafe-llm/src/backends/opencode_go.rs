use super::{LlmBackend, LlmMessage, LlmParams};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

/// Anthropic Messages requests require a `max_tokens`; use this when unset.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// OpenCode Go requires a client-identifying User-Agent (not a generic SDK
/// name) and a stable per-conversation `x-opencode-session` for routing and
/// prompt caching. See https://opencode.ai/docs/go/#where-can-i-use-it
const USER_AGENT: &str = concat!("cafe-llm/", env!("CARGO_PKG_VERSION"));
const SESSION_HEADER: &str = "x-opencode-session";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_SESSION: &str = "cafe-llm";

/// Wire protocol an OpenCode Go model is served over. The gateway exposes the
/// same model catalog across three different APIs; the model ID determines
/// which one to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// OpenAI Chat Completions (`/v1/chat/completions`) — DeepSeek, GLM, Kimi,
    /// MiMo, LongCat, Hy.
    ChatCompletions,
    /// OpenAI Responses (`/v1/responses`) — Grok, GPT Luna, Muse Spark.
    Responses,
    /// Anthropic Messages (`/v1/messages`) — MiniMax, Qwen.
    Messages,
}

/// Route a model ID to its OpenCode Go wire protocol. Unknown models default
/// to Chat Completions, which serves the largest part of the catalog.
pub fn protocol_for_model(model: &str) -> Protocol {
    let m = model.to_ascii_lowercase();
    if m.starts_with("grok") || m.starts_with("gpt-") || m.starts_with("muse-spark") {
        Protocol::Responses
    } else if m.starts_with("minimax") || m.starts_with("qwen") {
        Protocol::Messages
    } else {
        Protocol::ChatCompletions
    }
}

pub struct OpenCodeGoBackend {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenCodeGoBackend {
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[async_trait]
impl LlmBackend for OpenCodeGoBackend {
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        params: &LlmParams,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let protocol = protocol_for_model(&params.model);
        let (path, body) = match protocol {
            Protocol::ChatCompletions => ("/v1/chat/completions", chat_body(messages, params)),
            Protocol::Responses => ("/v1/responses", responses_body(messages, params)),
            Protocol::Messages => ("/v1/messages", messages_body(messages, params)),
        };

        let session = params
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SESSION);

        let mut req = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(SESSION_HEADER, session)
            .json(&body);

        // The gateway authenticates Chat Completions / Responses with a bearer
        // token but the Anthropic-compatible endpoint expects `x-api-key`.
        req = match protocol {
            Protocol::Messages => req
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            Protocol::ChatCompletions | Protocol::Responses => req.bearer_auth(&self.api_key),
        };

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("OpenCode Go error {}: {}", status, text));
        }

        let byte_stream = response.bytes_stream();

        let token_stream = byte_stream
            .scan(Vec::<u8>::new(), move |buf, result| {
                let outcome: Result<String> = match result {
                    Err(e) => Err(anyhow!("OpenCode Go stream error: {e}")),
                    Ok(bytes) => {
                        buf.extend_from_slice(&bytes);
                        let mut text = String::new();
                        let mut error: Option<anyhow::Error> = None;
                        while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                            let frame = buf.drain(..pos + 2).collect::<Vec<_>>();
                            let frame = String::from_utf8_lossy(&frame);
                            match frame_text(protocol, &frame) {
                                Ok(Some(delta)) => text.push_str(&delta),
                                Ok(None) => {}
                                Err(e) => {
                                    error = Some(e);
                                    break;
                                }
                            }
                        }
                        match error {
                            Some(e) => Err(e),
                            None => Ok(text),
                        }
                    }
                };
                futures_util::future::ready(Some(outcome))
            })
            .filter(|r| {
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                futures_util::future::ready(keep)
            });

        Ok(Box::pin(token_stream))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let list: ModelList = resp.json().await?;
        let mut models: Vec<String> = list.data.into_iter().map(|m| m.id).collect();
        models.sort();
        models.dedup();
        Ok(models)
    }
}

fn chat_body(messages: Vec<LlmMessage>, params: &LlmParams) -> Value {
    let msgs: Vec<Value> = messages
        .into_iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    let mut body = json!({
        "model": params.model,
        "messages": msgs,
        "stream": true,
    });
    if let Some(t) = params.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(mt) = params.max_tokens {
        body["max_tokens"] = json!(mt);
    }
    body
}

/// OpenAI Responses API body. `system` messages become the top-level
/// `instructions` string; other turns become `input` items.
fn responses_body(messages: Vec<LlmMessage>, params: &LlmParams) -> Value {
    let mut instructions: Option<String> = None;
    let mut input: Vec<Value> = Vec::new();
    for m in messages {
        if m.role == "system" {
            push_system(&mut instructions, &m.content);
        } else {
            input.push(json!({ "role": m.role, "content": m.content }));
        }
    }
    let mut body = json!({
        "model": params.model,
        "input": input,
        "stream": true,
    });
    if let Some(s) = instructions {
        body["instructions"] = json!(s);
    }
    if let Some(t) = params.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(mt) = params.max_tokens {
        body["max_output_tokens"] = json!(mt);
    }
    body
}

/// Anthropic Messages body. `system` is a top-level string field, not a
/// message; temperature is clamped to Anthropic's `[0, 1]` range.
fn messages_body(messages: Vec<LlmMessage>, params: &LlmParams) -> Value {
    let mut system: Option<String> = None;
    let mut msgs: Vec<Value> = Vec::new();
    for m in messages {
        if m.role == "system" {
            push_system(&mut system, &m.content);
        } else {
            msgs.push(json!({ "role": m.role, "content": m.content }));
        }
    }
    let mut body = json!({
        "model": params.model,
        "max_tokens": params.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": msgs,
        "stream": true,
    });
    if let Some(s) = system {
        body["system"] = json!(s);
    }
    if let Some(t) = params.temperature {
        body["temperature"] = json!(t.clamp(0.0, 1.0));
    }
    body
}

fn push_system(slot: &mut Option<String>, content: &str) {
    match slot {
        Some(existing) => {
            existing.push('\n');
            existing.push_str(content);
        }
        None => *slot = Some(content.to_string()),
    }
}

/// Extract streamed assistant text from one SSE frame, or an error if the
/// frame carries an OpenCode Go error event.
fn frame_text(protocol: Protocol, frame: &str) -> Result<Option<String>> {
    let mut text = String::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if val.get("type").and_then(Value::as_str) == Some("error") || !val["error"].is_null() {
            let msg = val
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| val.get("error").and_then(Value::as_str))
                .unwrap_or("unknown error");
            return Err(anyhow!("OpenCode Go stream error: {msg}"));
        }
        if let Some(delta) = extract_delta(protocol, &val) {
            text.push_str(&delta);
        }
    }
    Ok(if text.is_empty() { None } else { Some(text) })
}

/// Pull the assistant-text delta out of a single decoded SSE payload.
fn extract_delta(protocol: Protocol, val: &Value) -> Option<String> {
    match protocol {
        Protocol::ChatCompletions => val["choices"][0]["delta"]["content"]
            .as_str()
            .map(String::from),
        Protocol::Responses => {
            if val["type"].as_str() == Some("response.output_text.delta") {
                val["delta"].as_str().map(String::from)
            } else {
                None
            }
        }
        Protocol::Messages => {
            if val["type"].as_str() == Some("content_block_delta")
                && val["delta"]["type"].as_str() == Some("text_delta")
            {
                val["delta"]["text"].as_str().map(String::from)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(model: &str) -> LlmParams {
        LlmParams {
            model: model.to_string(),
            temperature: None,
            max_tokens: None,
            session_id: Some("sess-123".into()),
            backend: None,
        }
    }

    fn msgs() -> Vec<LlmMessage> {
        vec![
            LlmMessage {
                role: "system".into(),
                content: "be terse".into(),
            },
            LlmMessage {
                role: "user".into(),
                content: "hi".into(),
            },
            LlmMessage {
                role: "assistant".into(),
                content: "hello".into(),
            },
            LlmMessage {
                role: "user".into(),
                content: "bye".into(),
            },
        ]
    }

    #[test]
    fn routes_models_to_protocols() {
        assert_eq!(
            protocol_for_model("deepseek-v4.1-flash"),
            Protocol::ChatCompletions
        );
        assert_eq!(protocol_for_model("glm-5.3"), Protocol::ChatCompletions);
        assert_eq!(
            protocol_for_model("kimi-k2.7-code"),
            Protocol::ChatCompletions
        );
        assert_eq!(protocol_for_model("grok-4.6"), Protocol::Responses);
        assert_eq!(protocol_for_model("grok-4.7"), Protocol::Responses);
        assert_eq!(protocol_for_model("gpt-6-luna"), Protocol::Responses);
        assert_eq!(
            protocol_for_model("muse-spark-1.2-contributor"),
            Protocol::Responses
        );
        assert_eq!(protocol_for_model("minimax-m3"), Protocol::Messages);
        assert_eq!(protocol_for_model("qwen3.8-max"), Protocol::Messages);
        // Case-insensitive and unknown-model default.
        assert_eq!(protocol_for_model("GROK-4.6"), Protocol::Responses);
        assert_eq!(
            protocol_for_model("some-new-model"),
            Protocol::ChatCompletions
        );
    }

    #[test]
    fn chat_body_keeps_system_message() {
        let body = chat_body(msgs(), &params("deepseek-v4.1-flash"));
        assert_eq!(body["model"], "deepseek-v4.1-flash");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be terse");
        assert_eq!(body["messages"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn responses_body_moves_system_to_instructions() {
        let body = responses_body(msgs(), &params("grok-4.6"));
        assert_eq!(body["instructions"], "be terse");
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3, "system must not remain in input");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "hi");
    }

    #[test]
    fn messages_body_uses_top_level_system_and_default_max_tokens() {
        let body = messages_body(msgs(), &params("minimax-m3"));
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3, "system must not remain in messages");
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn messages_body_preserves_explicit_max_tokens_and_clamps_temperature() {
        let mut p = params("qwen3.8-max");
        p.max_tokens = Some(128);
        p.temperature = Some(1.7);
        let body = messages_body(msgs(), &p);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(
            body["temperature"], 1.0,
            "Anthropic temperature clamps at 1.0"
        );
    }

    #[test]
    fn extracts_text_from_chat_completions() {
        let val: Value =
            serde_json::from_str(r#"{"choices":[{"delta":{"content":"OK"}}]}"#).unwrap();
        assert_eq!(
            extract_delta(Protocol::ChatCompletions, &val).as_deref(),
            Some("OK")
        );
    }

    #[test]
    fn extracts_text_from_responses_and_ignores_reasoning() {
        let text: Value =
            serde_json::from_str(r#"{"type":"response.output_text.delta","delta":"OK"}"#).unwrap();
        assert_eq!(
            extract_delta(Protocol::Responses, &text).as_deref(),
            Some("OK")
        );

        let reasoning: Value = serde_json::from_str(
            r#"{"type":"response.reasoning_summary_text.delta","delta":"think"}"#,
        )
        .unwrap();
        assert_eq!(extract_delta(Protocol::Responses, &reasoning), None);
    }

    #[test]
    fn extracts_text_from_anthropic_and_ignores_thinking() {
        let text: Value = serde_json::from_str(
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"OK"}}"#,
        )
        .unwrap();
        assert_eq!(
            extract_delta(Protocol::Messages, &text).as_deref(),
            Some("OK")
        );

        let thinking: Value = serde_json::from_str(
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
        )
        .unwrap();
        assert_eq!(extract_delta(Protocol::Messages, &thinking), None);
    }

    #[test]
    fn frame_text_collects_multiline_data_and_skips_done() {
        let frame = "event: response.output_text.delta\n\
                     data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello \"}\n\
                     \n\
                     data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n\
                     data: [DONE]\n";
        assert_eq!(
            frame_text(Protocol::Responses, frame).unwrap().as_deref(),
            Some("Hello world")
        );
    }

    #[test]
    fn frame_text_surfaces_error_events() {
        let frame = "data: {\"type\":\"error\",\"error\":{\"type\":\"AuthError\",\"message\":\"Missing API key.\"}}\n";
        let err = frame_text(Protocol::Messages, frame).unwrap_err();
        assert!(err.to_string().contains("Missing API key"), "{err}");
    }

    #[test]
    fn frame_text_surfaces_bare_error_object() {
        let frame = "data: {\"error\":{\"message\":\"rate limited\"}}\n";
        let err = frame_text(Protocol::ChatCompletions, frame).unwrap_err();
        assert!(err.to_string().contains("rate limited"), "{err}");
    }
}
