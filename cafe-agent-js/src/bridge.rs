use anyhow::{Context as _, Result};
use cafe_sdk::bus::BusClient;
use cafe_sdk::{
    keys, roles, Chunk, ContentType, JsonRpcRequest, ServerMessage, ToolResult,
};
use rquickjs::{function::Async, AsyncContext, AsyncRuntime, Function, Object, Promise};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::info;

// ---------------------------------------------------------------------------
// JsEvent — bus chunk classified into the vocabulary JS agents speak
// ---------------------------------------------------------------------------

/// A single event delivered to a JS agent. Serialized as JSON and handed to
/// `cafe.nextEvent()`; `cafe.events()` (defined in [`JS_PRELUDE`]) yields these
/// one at a time so agent code is a plain `for await` loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct JsEvent {
    /// `user_message` | `llm_complete` | `tick`
    #[serde(rename = "type")]
    pub event_type: String,
    /// User/assistant text (empty for ticks and binary events).
    pub text: String,
}

/// Assemble the latest assistant text from `history` for `llm_complete`
/// events, whose trigger chunk (the `stream_complete` marker) carries no
/// content itself. Ports `extract_last_assistant_text` from
/// `cafe-agent-runtime/src/session_loop.rs`.
pub fn assemble_llm_text(history: &[Chunk]) -> Option<String> {
    for chunk in history.iter().rev() {
        if chunk.content_type == ContentType::Text
            && chunk.role() == Some(roles::ASSISTANT)
            && chunk.producer == "com.nominal.cafe-llm"
        {
            if let Some(ref text) = chunk.content {
                if !text.is_empty() {
                    return Some(text.clone());
                }
            }
        }
    }
    None
}

/// Classify a bus chunk into a [`JsEvent`]. Returns `None` for chunks that
/// must not wake the agent (transient RPC plumbing, unrelated roles, …).
/// Mirrors the trigger routing in `cafe-agent-runtime/src/session_loop.rs`.
///
/// NOTE: `llm_complete` events carry empty `text` here — the supervisor
/// fills it via [`assemble_llm_text`], since the trigger chunk is the
/// content-less `stream_complete` marker.
pub fn classify_event(chunk: &Chunk) -> Option<JsEvent> {
    if chunk.is_transient() {
        return None;
    }
    // User message (text or binary_ref) → user_message
    if chunk.role() == Some(roles::USER)
        && matches!(
            chunk.content_type,
            ContentType::Text | ContentType::BinaryRef
        )
    {
        return Some(JsEvent {
            event_type: "user_message".into(),
            text: chunk.content.clone().unwrap_or_default(),
        });
    }
    // LLM final chunk (stream_complete, non-transient, assistant) → llm_complete
    if chunk.role() == Some(roles::ASSISTANT)
        && chunk
            .get_annotation::<bool>(keys::CHAT_STREAM_COMPLETE)
            .unwrap_or(false)
    {
        return Some(JsEvent {
            event_type: "llm_complete".into(),
            text: chunk.content.clone().unwrap_or_default(),
        });
    }
    // Scheduler tick → tick
    if chunk
        .get_annotation::<String>(keys::CAFE_FLOW_SIGNAL)
        .as_deref()
        == Some("tick")
    {
        return Some(JsEvent {
            event_type: "tick".into(),
            text: String::new(),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// JS prelude — idiomatic surface over the string-based Rust bridge
// ---------------------------------------------------------------------------

/// Installed after the Rust `cafe` object. Defines:
/// - `cafe.events()` — async generator yielding parsed events until the
///   stream ends (`nextEvent()` returns null).
/// - `cafe.invoke(evaluator, params)` — `{evaluator}.invoke` RPC, rejects on
///   error/timeout so agents use plain `try/catch` + `await`.
/// - `cafe.rpc(method, params)` — raw RPC for any bus method.
/// - `cafe.publishText(text)` — assistant text chunk, rejects on failure.
/// - `cafe.tool(name, params)` — bus-RPC tool with `cafe.tool.result` +
///   readable text published for follow-up LLM steps.
/// - `cafe.config()` — merged runtime config snapshot (see [`resolve_config`]).
const JS_PRELUDE: &str = r#"
cafe.events = async function* () {
  while (true) {
    const raw = await cafe.nextEvent();
    if (raw === null || raw === undefined) return;
    yield JSON.parse(raw);
  }
};
cafe._unwrap = async function (envelopeJson) {
  const env = JSON.parse(envelopeJson);
  if (env.ok) return env.result;
  const err = new Error(env.error || "rpc failed");
  if (env.code !== undefined && env.code !== null) err.code = env.code;
  throw err;
};
cafe.invoke = async (evaluator, params) =>
  cafe._unwrap(await cafe._invoke(evaluator + ".invoke", JSON.stringify(params || {})));
cafe.rpc = async (method, params) =>
  cafe._unwrap(await cafe._invoke(method, JSON.stringify(params || {})));
cafe.publishText = async (text) =>
  cafe._unwrap(await cafe._publishText(text));
cafe.tool = async (name, params) =>
  cafe._unwrap(await cafe._tool(name, JSON.stringify(params || {})));
cafe.config = async () =>
  JSON.parse(await cafe._config());

// --- HTML5-style fetch -----------------------------------------------------
// Not the real WHATWG implementation, but the common shape: resolves on any
// HTTP status, rejects only on network/transport errors.
function normHeaders(h) {
  if (!h) return {};
  if (Array.isArray(h)) {
    const out = {};
    for (const pair of h) out[pair[0]] = pair[1];
    return out;
  }
  if (typeof h.entries === "function") {
    const out = {};
    for (const pair of h.entries()) out[pair[0]] = pair[1];
    return out;
  }
  return h;
}
cafe._headers = function (map) {
  const lower = {};
  for (const key of Object.keys(map || {})) lower[key.toLowerCase()] = map[key];
  return {
    get: (name) => (name in lower ? lower[String(name).toLowerCase()] : null),
    has: (name) => String(name).toLowerCase() in lower,
    forEach: (cb) => { for (const k of Object.keys(lower)) cb(lower[k], k); },
    entries: () => Object.entries(lower),
    keys: () => Object.keys(lower),
    values: () => Object.values(lower),
  };
};
cafe.fetch = async function (url, options) {
  const opts = options || {};
  const env = JSON.parse(await cafe._fetch(String(url), JSON.stringify({
    method: opts.method || "GET",
    headers: normHeaders(opts.headers),
    body: opts.body === undefined || opts.body === null ? null : String(opts.body),
  })));
  if (!env.ok) throw new TypeError("fetch failed: " + (env.error || "unknown error"));
  const r = env.result;
  return {
    ok: r.ok,
    status: r.status,
    statusText: r.statusText,
    url: r.url,
    headers: cafe._headers(r.headers),
    text: async () => r.body,
    json: async () => JSON.parse(r.body),
  };
};
if (typeof globalThis !== "undefined") globalThis.fetch = cafe.fetch;
"#;

// ---------------------------------------------------------------------------
// RPC — publish transient request, await matching response by call_id
// ---------------------------------------------------------------------------

/// Envelope returned (as a JSON string) by every `_invoke` / `_publishText`
/// bridge call. JS unwraps it and throws a real `Error` on `ok: false`, so
/// Rust never needs to construct a QuickJS exception — domain errors travel
/// as data, and `rquickjs::Error` only surfaces for engine-level failures.
fn ok_envelope(result: serde_json::Value) -> String {
    serde_json::json!({ "ok": true, "result": result }).to_string()
}

fn err_envelope(code: Option<i32>, message: impl Into<String>) -> String {
    let mut env = serde_json::json!({ "ok": false, "error": message.into() });
    if let Some(c) = code {
        env["code"] = serde_json::json!(c);
    }
    env.to_string()
}

/// Internal outcome of one JSON-RPC call, before envelope wrapping.
enum RpcOutcome {
    Ok(serde_json::Value),
    Err { code: Option<i32>, message: String },
}

/// Publish a `{method}` JSON-RPC request on `session_id` and resolve with the
/// response whose `id` matches the request's `call_id` (ADR-006). Concurrent
/// awaits are safe: every call mints its own UUID and filters only for it.
async fn rpc_call(
    client: &BusClient,
    session_id: &str,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> RpcOutcome {
    // Subscribe BEFORE publishing so the response cannot slip past us.
    let mut sub = match client.subscribe_session(session_id).await {
        Ok(s) => s,
        Err(e) => {
            return RpcOutcome::Err {
                code: None,
                message: format!("subscribe failed: {e}"),
            }
        }
    };
    let request = JsonRpcRequest::new(method, params);
    let call_id = request.id.clone();
    let req_chunk = Chunk::new_null("com.nominal.cafe-agent-js")
        .with_annotation(keys::CAFE_JSONRPC_REQUEST, &request)
        .as_transient()
        .with_retain(60);
    if let Err(e) = sub.publish(req_chunk).await {
        return RpcOutcome::Err {
            code: None,
            message: format!("publish failed: {e}"),
        };
    }
    let outcome = tokio::time::timeout(timeout, async {
        loop {
            match sub.rx.recv().await {
                Some(ServerMessage::Chunk { chunk, .. }) => {
                    if chunk.is_rpc_response_for(&call_id) {
                        return chunk.as_rpc_response().ok_or_else(|| {
                            anyhow::anyhow!("response for {call_id} failed to deserialize")
                        });
                    }
                }
                Some(_) => continue,
                None => anyhow::bail!("bus disconnected while awaiting {method}"),
            }
        }
    })
    .await;
    match outcome {
        Err(_) => RpcOutcome::Err {
            code: None,
            message: format!("{method} timed out after {}s", timeout.as_secs()),
        },
        Ok(Err(e)) => RpcOutcome::Err {
            code: None,
            message: e.to_string(),
        },
        Ok(Ok(resp)) => match resp.error {
            Some(err) => RpcOutcome::Err {
                code: Some(err.code),
                message: err.message,
            },
            None => RpcOutcome::Ok(resp.result.unwrap_or(serde_json::Value::Null)),
        },
    }
}

/// Envelope wrapper for raw RPC (`cafe.rpc` / `cafe.invoke`).
async fn rpc_roundtrip(
    client: &BusClient,
    session_id: &str,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> String {
    match rpc_call(client, session_id, method, params, timeout).await {
        RpcOutcome::Ok(result) => ok_envelope(result),
        RpcOutcome::Err { code, message } => err_envelope(code, message),
    }
}

/// Fire-and-forget chunk publish (no reply expected; the one-shot `publish`
/// deprecation does not apply).
async fn publish_one_shot(client: &BusClient, session_id: &str, chunk: Chunk) {
    #[allow(deprecated)]
    if let Err(e) = client.publish(session_id, chunk).await {
        tracing::warn!("cafe-agent-js: one-shot publish failed: {e}");
    }
}

/// Execute a bus-RPC tool (`cafe.tool`), mirroring `tool_executor::execute`
/// but awaitable: dispatch `{name}` by `call_id`, then publish the
/// bus-visible `cafe.tool.result` chunk plus the human-readable assistant
/// text the follow-up LLM reads. Returns the tool output.
///
/// Bus-RPC only: MCP-provided tools (`provider: "mcp"`) stay with
/// `cafe-mcp-client` and have no direct RPC method to call.
async fn tool_roundtrip(
    client: &BusClient,
    session_id: &str,
    name: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> String {
    match rpc_call(client, session_id, name, params, timeout).await {
        RpcOutcome::Ok(output) => {
            let result = ToolResult {
                name: name.to_string(),
                output: output.clone(),
                error: None,
                provider: None,
            };
            publish_one_shot(
                client,
                session_id,
                Chunk::new_null("com.nominal.cafe-agent-js")
                    .with_annotation(keys::CAFE_TOOL_RESULT, &result)
                    .as_transient()
                    .with_retain(60),
            )
            .await;
            let output_text = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".into());
            publish_one_shot(
                client,
                session_id,
                Chunk::new_text(
                    format!("Tool call completed: {name}\n```\n{output_text}\n```"),
                    "com.nominal.cafe-agent-js",
                )
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT),
            )
            .await;
            info!("cafe-agent-js: tool {name} succeeded");
            ok_envelope(output)
        }
        RpcOutcome::Err { code, message } => {
            let failed_text = if message.contains("timed out") {
                format!("Tool call {name} timed out")
            } else {
                format!("Tool call {name} failed: {message}")
            };
            let result = ToolResult {
                name: name.to_string(),
                output: serde_json::Value::Null,
                error: Some(message.clone()),
                provider: None,
            };
            publish_one_shot(
                client,
                session_id,
                Chunk::new_null("com.nominal.cafe-agent-js")
                    .with_annotation(keys::CAFE_TOOL_RESULT, &result)
                    .as_transient()
                    .with_retain(60),
            )
            .await;
            publish_one_shot(
                client,
                session_id,
                Chunk::new_text(&failed_text, "com.nominal.cafe-agent-js")
                    .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT),
            )
            .await;
            err_envelope(code, message)
        }
    }
}

// ---------------------------------------------------------------------------
// Outbound HTTP — `cafe.fetch`
// ---------------------------------------------------------------------------

/// Perform an outbound HTTP request on behalf of a JS agent.
///
/// Returns an envelope whose `result` is
/// `{ ok, status, statusText, url, headers, body }`. HTTP error statuses are
/// a *resolved* result (`ok: false`), matching the browser Fetch API; only
/// transport failures produce `ok: false` at the envelope level, which the
/// prelude turns into a rejected `TypeError`.
///
/// Agents are trusted local code, but this gives them the host's network
/// egress — see ADR-131.
async fn http_fetch(url: String, options_json: String, timeout: Duration) -> String {
    let opts: serde_json::Value =
        serde_json::from_str(&options_json).unwrap_or(serde_json::Value::Null);
    let method = opts["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
    let body = opts["body"].as_str().map(str::to_string);

    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => return err_envelope(None, format!("http client error: {e}")),
    };
    let method = match reqwest::Method::from_bytes(method.as_bytes()) {
        Ok(m) => m,
        Err(_) => return err_envelope(None, format!("invalid HTTP method: {method}")),
    };
    let mut request = client.request(method, &url);
    if let Some(headers) = opts["headers"].as_object() {
        for (name, value) in headers {
            if let Some(value) = value.as_str() {
                request = request.header(name, value);
            }
        }
    }
    if let Some(body) = body {
        request = request.body(body);
    }

    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let final_url = response.url().to_string();
            let mut headers = serde_json::Map::new();
            for (name, value) in response.headers() {
                headers.insert(
                    name.as_str().to_string(),
                    serde_json::Value::String(value.to_str().unwrap_or("").to_string()),
                );
            }
            let body = response.text().await.unwrap_or_default();
            info!("cafe-agent-js: fetch {url} -> {}", status.as_u16());
            ok_envelope(serde_json::json!({
                "ok": status.is_success(),
                "status": status.as_u16(),
                "statusText": status.canonical_reason().unwrap_or(""),
                "url": final_url,
                "headers": headers,
                "body": body,
            }))
        }
        Err(e) => err_envelope(None, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Session config — merged runtime null chunks (mirrors config.rs semantics)
// ---------------------------------------------------------------------------

/// Merge all `config.type == "runtime"` null chunks in `history` into one
/// JSON object (later chunks win per key). This is the `SessionConfig.extra`
/// view flattened: every annotation except the marker itself.
pub fn resolve_config(history: &[Chunk]) -> serde_json::Value {
    let mut merged = serde_json::Map::new();
    for chunk in history {
        if chunk.content_type != ContentType::Null {
            continue;
        }
        if chunk
            .annotations
            .get(keys::CONFIG_TYPE)
            .and_then(|v| v.as_str())
            != Some("runtime")
        {
            continue;
        }
        for (key, value) in &chunk.annotations {
            if key == keys::CONFIG_TYPE {
                continue;
            }
            merged.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(merged)
}

/// Snapshot the session config for one agent run. History fetch failure
/// degrades to `{}` rather than failing the run — config is advisory, and
/// agents must still answer when the store is briefly unreachable.
pub(crate) async fn fetch_config_json(client: &BusClient, session_id: &str) -> String {
    match client.get_history(session_id).await {
        Ok(history) => resolve_config(&history).to_string(),
        Err(e) => {
            tracing::warn!("cafe-agent-js: config snapshot failed: {e}");
            "{}".into()
        }
    }
}

// ---------------------------------------------------------------------------
// Event streaming — one `cafe.events()` surface for both modes
// ---------------------------------------------------------------------------

/// Receiving end of a session's event stream, shared with the JS task.
/// Unbounded: events are small JSON strings, and backpressure must never
/// stall the bus loop (a wedged agent would otherwise wedge the session).
pub(crate) type EventRx = Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<String>>>;

/// Live config snapshot, refreshed by the supervisor on every event and read
/// by `cafe.config()`. Stateless runs fill it once; stateful runs refresh it.
pub(crate) type ConfigSlot = Arc<Mutex<String>>;

pub(crate) fn new_event_channel() -> (
    tokio::sync::mpsc::UnboundedSender<String>,
    EventRx,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (tx, Arc::new(Mutex::new(rx)))
}

// ---------------------------------------------------------------------------
// run_js_source / run_js_stream — stateless per-event vs stateful session
// ---------------------------------------------------------------------------

/// Overall budget for one stateless agent run (covers JS execution + any RPC
/// awaits). Stateful runs have no overall budget — they live as long as the
/// session does; each individual RPC await is still bounded by `rpc_timeout`.
const JS_BUDGET_SECS: u64 = 120;

/// Run file-loaded agent `source` for a single `event` in a fresh QuickJS
/// runtime (stateless mode: no JS state survives between events — all durable
/// state lives in bus history).
///
/// Returns whatever the agent's `main()` returned.
pub(crate) async fn run_js_source(
    client: &BusClient,
    session_id: &str,
    agent_id: &str,
    event: &JsEvent,
    source: &str,
    rpc_timeout: Duration,
) -> Result<String> {
    let event_json =
        serde_json::to_string(event).context("failed to serialize JsEvent")?;
    let (tx, rx) = new_event_channel();
    tx.send(event_json)
        .map_err(|_| anyhow::anyhow!("event channel closed"))?;
    drop(tx); // Generator ends after the single event.
    let config: ConfigSlot = Arc::new(Mutex::new(
        fetch_config_json(client, session_id).await,
    ));
    let client = client.clone();
    let session_id = session_id.to_string();
    let agent_id = agent_id.to_string();
    let source = source.to_string();

    tokio::time::timeout(
        Duration::from_secs(JS_BUDGET_SECS),
        run_js_stream(&client, &session_id, &agent_id, &source, rpc_timeout, rx, config),
    )
    .await
    .map_err(|_| anyhow::anyhow!("js agent exceeded {JS_BUDGET_SECS}s budget"))?
}

/// Run agent `source` against a live event stream in a dedicated QuickJS
/// runtime (stateful mode). Resolves when `main(cafe)` settles — normally
/// when the supervisor drops the sender on session end, which closes the
/// `cafe.events()` generator. No overall timeout: omission of a timeout here
/// is deliberate (see `JS_BUDGET_SECS`).
pub(crate) async fn run_js_stream(
    client: &BusClient,
    session_id: &str,
    agent_id: &str,
    source: &str,
    rpc_timeout: Duration,
    event_rx: EventRx,
    config: ConfigSlot,
) -> Result<String> {
    let client = client.clone();
    let session_id = session_id.to_string();
    let agent_id = agent_id.to_string();
    let source = source.to_string();

    async move {
        let rt = AsyncRuntime::new().context("failed to create QuickJS runtime")?;
        let ctx = AsyncContext::full(&rt)
            .await
            .context("failed to create QuickJS context")?;
        ctx.async_with(async |ctx| {
            install_cafe(
                &ctx,
                &client,
                &session_id,
                &agent_id,
                event_rx,
                config,
                rpc_timeout,
            )?;
            ctx.eval::<(), _>(JS_PRELUDE)
                .map_err(|e| anyhow::anyhow!("prelude eval failed: {e}"))?;
            ctx.eval::<(), _>(source)
                .map_err(|e| anyhow::anyhow!("agent eval failed: {e}"))?;
            // JS-side settlement envelope: rejections become data carrying the
            // JS error message + stack, so Rust never sees a bare `Exception`.
            // `into_future` then only fails on engine-level breakdown.
            let promise: Promise = ctx
                .eval(
                    r#"(main(cafe).then(
                        v => JSON.stringify({ ok: true, value: v === undefined ? null : v }),
                        e => JSON.stringify({ ok: false, error: String((e && e.stack) || e) })
                    ))"#,
                )
                .map_err(|e| anyhow::anyhow!("failed to call main(cafe): {e}"))?;
            let out: String = promise
                .into_future()
                .await
                .map_err(|e| anyhow::anyhow!("js engine failure: {e}"))?;
            let env: serde_json::Value = serde_json::from_str(&out)
                .context("agent settlement was not valid JSON")?;
            if env.get("ok") == Some(&serde_json::Value::Bool(true)) {
                Ok(env
                    .get("value")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_default())
            } else {
                Err(anyhow::anyhow!(
                    "js agent failed: {}",
                    env.get("error").and_then(|v| v.as_str()).unwrap_or("unknown")
                ))
            }
        })
        .await
    }
    .await
}

/// Install the Rust `cafe` object into `ctx`:
/// plain data (`sessionId`, `agentId`), streaming `nextEvent()` (resolves
/// from the session's event channel; null when the sender is dropped, which
/// ends `cafe.events()`), promise RPC (`_invoke`), tool execution (`_tool`),
/// text publish (`_publishText`), live config (`_config`), and `log`.
///
/// All cross-boundary values are strings (JSON). Async callbacks take their
/// arguments as owned Rust values first, then perform bus I/O with no borrow
/// of the JS context held across `.await`.
fn install_cafe(
    ctx: &rquickjs::Ctx<'_>,
    client: &BusClient,
    session_id: &str,
    agent_id: &str,
    event_rx: EventRx,
    config: ConfigSlot,
    rpc_timeout: Duration,
) -> Result<()> {
    let cafe = Object::new(ctx.clone()).context("failed to create cafe object")?;
    cafe.set("sessionId", session_id.to_string())?;
    cafe.set("agentId", agent_id.to_string())?;

    // Streaming event supply shared with the supervisor. Stateless runs get
    // a channel holding exactly one event whose sender is already dropped;
    // stateful runs share a live channel fed per bus event. Same JS surface.
    let next_event = {
        let event_rx = event_rx.clone();
        Function::new(
            ctx.clone(),
            Async(move || {
                let event_rx = event_rx.clone();
                async move {
                    let mut rx = event_rx.lock().await;
                    Ok::<_, rquickjs::Error>(rx.recv().await)
                }
            }),
        )?
    };
    cafe.set("nextEvent", next_event)?;

    // Promise RPC: `cafe._invoke(method, paramsJson) -> envelopeJson`.
    // The returned future becomes a JS Promise; concurrent awaits multiplex
    // over per-call subscriptions, demultiplexed by call_id.
    let invoke = {
        let client = client.clone();
        let session_id = session_id.to_string();
        Function::new(
            ctx.clone(),
            Async(move |method: String, params_json: String| {
                let client = client.clone();
                let session_id = session_id.clone();
                async move {
                    let params: serde_json::Value =
                        serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
                    Ok::<_, rquickjs::Error>(
                        rpc_roundtrip(&client, &session_id, &method, params, rpc_timeout).await,
                    )
                }
            }),
        )?
    };
    cafe.set("_invoke", invoke)?;

    // Tool execution: `cafe._tool(name, paramsJson) -> envelopeJson`.
    // Like `_invoke` plus bus-visible `cafe.tool.result` and readable text
    // so a follow-up LLM step finds the result in history.
    let tool = {
        let client = client.clone();
        let session_id = session_id.to_string();
        Function::new(
            ctx.clone(),
            Async(move |name: String, params_json: String| {
                let client = client.clone();
                let session_id = session_id.clone();
                async move {
                    let params: serde_json::Value =
                        serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
                    Ok::<_, rquickjs::Error>(
                        tool_roundtrip(&client, &session_id, &name, params, rpc_timeout).await,
                    )
                }
            }),
        )?
    };
    cafe.set("_tool", tool)?;

    // Publish an assistant text chunk; envelope reports failure to JS.
    let publish_text = {
        let client = client.clone();
        let session_id = session_id.to_string();
        Function::new(
            ctx.clone(),
            Async(move |text: String| {
                let client = client.clone();
                let session_id = session_id.clone();
                async move {
                    let mut sub = match client.subscribe_session(&session_id).await {
                        Ok(s) => s,
                        Err(e) => {
                            return Ok::<_, rquickjs::Error>(err_envelope(
                                None,
                                format!("subscribe failed: {e}"),
                            ));
                        }
                    };
                    let chunk = Chunk::new_text(&text, "com.nominal.cafe-agent-js")
                        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
                    match sub.publish(chunk).await {
                        Ok(()) => {
                            info!("cafe-agent-js: published agent text ({})", text.len());
                            Ok(ok_envelope(serde_json::json!({ "published": true })))
                        }
                        Err(e) => Ok(err_envelope(None, format!("publish failed: {e}"))),
                    }
                }
            }),
        )?
    };
    cafe.set("_publishText", publish_text)?;

    // Live config slot: refreshed by the supervisor on every event
    // (stateless: filled once; stateful: updated per event).
    let config_fn = {
        let config = config.clone();
        Function::new(
            ctx.clone(),
            Async(move || {
                let config = config.clone();
                async move { Ok::<_, rquickjs::Error>(config.lock().await.clone()) }
            }),
        )?
    };
    cafe.set("_config", config_fn)?;

    // Outbound HTTP: `cafe._fetch(url, optionsJson) -> envelopeJson`.
    // The prelude shapes the result into a Fetch-API-like Response.
    let fetch = Function::new(
        ctx.clone(),
        Async(move |url: String, options_json: String| {
            async move { Ok::<_, rquickjs::Error>(http_fetch(url, options_json, rpc_timeout).await) }
        }),
    )?;
    cafe.set("_fetch", fetch)?;

    let log = Function::new(ctx.clone(), move |msg: String| {
        info!("cafe-agent-js [js]: {msg}");
    })?;
    cafe.set("log", log)?;

    ctx.globals()
        .set("cafe", cafe)
        .context("failed to install cafe global")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (no live bus required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn user_chunk(text: &str) -> Chunk {
        Chunk::new_text(text, "test").with_annotation(keys::CHAT_ROLE, roles::USER)
    }

    /// Test channel pre-filled with `events`; sender stays alive (returned)
    /// so the caller controls when the stream ends by dropping it.
    fn test_channel(
        events: &[&str],
        config_json: &str,
    ) -> (
        tokio::sync::mpsc::UnboundedSender<String>,
        EventRx,
        ConfigSlot,
    ) {
        let (tx, rx) = new_event_channel();
        for e in events {
            tx.send(e.to_string()).unwrap();
        }
        let slot: ConfigSlot = Arc::new(Mutex::new(config_json.to_string()));
        (tx, rx, slot)
    }

    /// One-shot channel (sender dropped): preserves the stateless semantic.
    fn oneshot_channel(event_json: &str, config_json: &str) -> (EventRx, ConfigSlot) {
        let (tx, rx, slot) = test_channel(&[event_json], config_json);
        drop(tx);
        (rx, slot)
    }

    #[test]
    fn classify_user_text_message() {
        let ev = classify_event(&user_chunk("hello")).unwrap();
        assert_eq!(
            ev,
            JsEvent {
                event_type: "user_message".into(),
                text: "hello".into(),
            }
        );
    }

    #[test]
    fn classify_ignores_transient_chunks() {
        let chunk = user_chunk("hello").as_transient();
        assert!(classify_event(&chunk).is_none());
    }

    #[test]
    fn classify_ignores_assistant_text_without_stream_complete() {
        let chunk = Chunk::new_text("partial", "test")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
        assert!(classify_event(&chunk).is_none());
    }

    #[test]
    fn classify_llm_complete_on_stream_complete() {
        let chunk = Chunk::new_text("done", "test")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
            .with_annotation(keys::CHAT_STREAM_COMPLETE, true);
        let ev = classify_event(&chunk).unwrap();
        assert_eq!(ev.event_type, "llm_complete");
        assert_eq!(ev.text, "done");
    }

    #[test]
    fn classify_scheduler_tick() {
        let chunk = Chunk::new_null("test").with_annotation(keys::CAFE_FLOW_SIGNAL, "tick");
        let ev = classify_event(&chunk).unwrap();
        assert_eq!(ev.event_type, "tick");
    }

    #[test]
    fn classify_returns_none_for_unrelated_chunks() {
        let chunk = Chunk::new_null("test");
        assert!(classify_event(&chunk).is_none());
    }

    #[test]
    fn assemble_llm_text_finds_last_assistant_text() {
        let history = vec![
            Chunk::new_text("old", "com.nominal.cafe-llm")
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT),
            user_chunk("question"),
            Chunk::new_text("new", "com.nominal.cafe-llm")
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT),
            Chunk::new_null("com.nominal.cafe-llm")
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
                .with_annotation(keys::CHAT_STREAM_COMPLETE, true),
        ];
        assert_eq!(assemble_llm_text(&history).as_deref(), Some("new"));
    }

    #[test]
    fn assemble_llm_text_ignores_other_producers_and_empty() {
        let history = vec![
            Chunk::new_text("not-llm", "com.nominal.cafe-rot13")
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT),
            Chunk::new_text("", "com.nominal.cafe-llm")
                .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT),
        ];
        assert!(assemble_llm_text(&history).is_none());
        assert!(assemble_llm_text(&[]).is_none());
    }

    /// The installed `cafe` object exposes exactly the surface `JS_PRELUDE`
    /// expects. No bus traffic happens — construction alone never connects.
    #[tokio::test]
    async fn cafe_global_shape_matches_prelude() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let shape: String = ctx
            .async_with(async |ctx| {
                let (event_rx, config_slot) = oneshot_channel(
                    r#"{"type":"user_message","text":"hi"}"#,
                    r#"{"config.js.tag":"tick"}"#,
                );
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(1),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                ctx.eval::<String, _>(
                    r#"JSON.stringify({
                        sessionId: cafe.sessionId,
                        nextEvent: typeof cafe.nextEvent,
                        events: typeof cafe.events,
                        invoke: typeof cafe.invoke,
                        rpc: typeof cafe.rpc,
                        publishText: typeof cafe.publishText,
                        tool: typeof cafe.tool,
                        fetch: typeof cafe.fetch,
                        globalFetch: typeof globalThis.fetch,
                        config: typeof cafe.config,
                        log: typeof cafe.log
                    })"#,
                )
                .unwrap()
            })
            .await;
        let v: serde_json::Value = serde_json::from_str(&shape).unwrap();
        assert_eq!(v["sessionId"], "sess-1");
        for key in [
            "nextEvent", "events", "invoke", "rpc", "publishText", "tool", "fetch",
            "globalFetch", "config", "log",
        ] {
            assert_eq!(v[key], "function", "{key} must be a function");
        }
    }

    /// `cafe.events()` yields the one-shot event then ends the generator.
    #[tokio::test]
    async fn events_generator_yields_once_then_ends() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let out: String = ctx
            .async_with(async |ctx| {
                let (event_rx, config_slot) = oneshot_channel(
                    r#"{"type":"user_message","text":"hi"}"#,
                    r#"{"config.js.tag":"tick"}"#,
                );
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(1),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                let promise: Promise = ctx
                    .eval(
                        r#"(async () => {
                            const seen = [];
                            for await (const e of cafe.events()) seen.push(e.type + ":" + e.text);
                            return JSON.stringify(seen);
                        })()"#,
                    )
                    .unwrap();
                promise.into_future::<String>().await.unwrap()
            })
            .await;
        assert_eq!(out, r#"["user_message:hi"]"#);
    }

    /// A live channel yields many events; dropping the sender ends the stream.
    /// This is the stateful transport — same `cafe.events()` surface.
    #[tokio::test]
    async fn events_stream_yields_many_then_ends_on_sender_drop() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let out: String = ctx
            .async_with(async |ctx| {
                let (tx, event_rx, config_slot) = test_channel(
                    &[
                        r#"{"type":"user_message","text":"one"}"#,
                        r#"{"type":"user_message","text":"two"}"#,
                        r#"{"type":"tick","text":""}"#,
                    ],
                    "{}",
                );
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(1),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                let promise: Promise = ctx
                    .eval(
                        r#"(async () => {
                            const seen = [];
                            for await (const e of cafe.events()) seen.push(e.type + ":" + e.text);
                            return JSON.stringify(seen);
                        })()"#,
                    )
                    .unwrap();
                // End the stream only after the consumer is waiting.
                let h = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    drop(tx);
                });
                let out = promise.into_future::<String>().await.unwrap();
                h.await.unwrap();
                out
            })
            .await;
        assert_eq!(out, r#"["user_message:one","user_message:two","tick:"]"#);
    }

    /// The Phase 3 discriminator: JS locals survive across events in one
    /// runtime. A stateless run would answer 1 every time; the stream answers
    /// 1 then 2.
    #[tokio::test]
    async fn run_js_stream_counter_accumulates_state_across_events() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let (tx, event_rx, config_slot) = test_channel(
            &[
                r#"{"type":"user_message","text":"a"}"#,
                r#"{"type":"user_message","text":"b"}"#,
            ],
            "{}",
        );
        let run = run_js_stream(
            &client,
            "sess-1",
            "counter",
            r#"
async function main(cafe) {
  let n = 0;
  for await (const event of cafe.events()) {
    if (event.type === "user_message") n++;
  }
  return "count=" + n;
}
"#,
            Duration::from_secs(2),
            event_rx,
            config_slot,
        );
        let h = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(tx);
        });
        let out = run.await.unwrap();
        h.await.unwrap();
        assert_eq!(out, "count=2");
    }

    /// A config slot update while the agent is parked awaiting the next
    /// event is visible on its next `cafe.config()` call. Deterministic:
    /// the agent cannot pass event 1 without event 2, which only arrives
    /// after the slot update.
    #[tokio::test]
    async fn config_slot_refresh_visible_to_running_agent() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let (tx, event_rx, config_slot) =
            test_channel(&[r#"{"type":"tick","text":""}"#], r#"{"v":"one"}"#);
        let run = run_js_stream(
            &client,
            "sess-1",
            "cfg",
            r#"
async function main(cafe) {
  const seen = [];
  for await (const event of cafe.events()) {
    const c = await cafe.config();
    seen.push(c.v);
  }
  return seen.join(",");
}
"#,
            Duration::from_secs(5),
            event_rx,
            config_slot.clone(),
        );
        let h = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            *config_slot.lock().await = r#"{"v":"two"}"#.to_string();
            tx.send(r#"{"type":"tick","text":""}"#.to_string())
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });
        let out = run.await.unwrap();
        h.await.unwrap();
        assert_eq!(out, "one,two");
    }

    /// Promise rejection path: `_invoke` against a dead socket resolves to an
    /// `ok:false` envelope, and `cafe.invoke` throws a real JS `Error`.
    #[tokio::test]
    async fn invoke_rejects_when_bus_unreachable() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let out: String = ctx
            .async_with(async |ctx| {
                let (event_rx, config_slot) = oneshot_channel(
                    r#"{"type":"user_message","text":"hi"}"#,
                    r#"{"config.js.tag":"tick"}"#,
                );
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(2),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                let promise: Promise = ctx
                    .eval(
                        r#"(async () => {
                            try {
                                await cafe.invoke("rot13", { text: "hi" });
                                return "NO-THROW";
                            } catch (e) {
                                return "THREW:" + e.message;
                            }
                        })()"#,
                    )
                    .unwrap();
                promise.into_future::<String>().await.unwrap()
            })
            .await;
        assert!(
            out.starts_with("THREW:"),
            "invoke must reject when the bus is unreachable, got: {out}"
        );
        // The rejection must carry the RPC envelope error (e.g. subscribe
        // failure) — not a SyntaxError from mishandling the promise. This
        // guards the prelude's await-before-parse contract.
        assert!(
            out.contains("failed"),
            "rejection must be the envelope error, got: {out}"
        );
        assert!(
            !out.contains("SyntaxError") && !out.contains("JSON"),
            "rejection must not be a JSON parsing artefact, got: {out}"
        );
    }

    /// `cafe.tool` rejects with the envelope error when the bus is
    /// unreachable — same contract as `cafe.invoke`.
    #[tokio::test]
    async fn tool_rejects_when_bus_unreachable() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let out: String = ctx
            .async_with(async |ctx| {
                let (tx, event_rx, config_slot) =
                    test_channel(&[r#"{"type":"user_message","text":"hi"}"#], "{}");
                drop(tx);
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(2),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                let promise: Promise = ctx
                    .eval(
                        r#"(async () => {
                            try {
                                await cafe.tool("dice.roll", { count: 2, sides: 6 });
                                return "NO-THROW";
                            } catch (e) {
                                return "THREW:" + e.message;
                            }
                        })()"#,
                    )
                    .unwrap();
                promise.into_future::<String>().await.unwrap()
            })
            .await;
        assert!(
            out.starts_with("THREW:") && out.contains("failed"),
            "tool must reject with the envelope error, got: {out}"
        );
        assert!(
            !out.contains("SyntaxError"),
            "rejection must not be a JSON parsing artefact, got: {out}"
        );
    }

    /// `fetch` rejects with a `TypeError` on transport errors, like the
    /// browser Fetch API (it resolves for HTTP error *statuses* instead).
    #[tokio::test]
    async fn fetch_rejects_on_network_error() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let out: String = ctx
            .async_with(async |ctx| {
                let (tx, event_rx, config_slot) =
                    test_channel(&[r#"{"type":"user_message","text":"hi"}"#], "{}");
                drop(tx);
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(2),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                let promise: Promise = ctx
                    .eval(
                        r#"(async () => {
                            try {
                                await cafe.fetch("http://127.0.0.1:1/");
                                return "NO-THROW";
                            } catch (e) {
                                return e.name + ":" + e.message;
                            }
                        })()"#,
                    )
                    .unwrap();
                promise.into_future::<String>().await.unwrap()
            })
            .await;
        assert!(
            out.starts_with("TypeError:"),
            "fetch must reject with TypeError on network error, got: {out}"
        );
    }

    /// End-to-end failure propagation: an inline agent's `await cafe.invoke`
    /// rejects on a dead socket, `main` throws, and `run_js_source` surfaces
    /// the JS exception message as a Rust error.
    #[tokio::test]
    async fn agent_source_surfaces_js_failure_as_rust_error() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let err = run_js_source(
            &client,
            "sess-1",
            "js-demo",
            &JsEvent {
                event_type: "user_message".into(),
                text: "hello".into(),
            },
            r#"
async function main(cafe) {
  for await (const event of cafe.events()) {
    const res = await cafe.invoke("rot13", { text: event.text });
    await cafe.publishText(res.text);
  }
  return "done";
}
"#,
            Duration::from_secs(2),
        )
        .await
        .expect_err("agent must fail without a live bus");
        let msg = err.to_string();
        assert!(
            msg.contains("js agent failed"),
            "error must be attributed to the JS agent, got: {msg}"
        );
    }

    /// `cafe.config()` resolves the snapshot installed by the host.
    #[tokio::test]
    async fn config_snapshot_reaches_js() {
        let client = BusClient::unix("/tmp/nonexistent-cafe-bus.sock");
        let rt = AsyncRuntime::new().unwrap();
        let ctx = AsyncContext::full(&rt).await.unwrap();
        let out: String = ctx
            .async_with(async |ctx| {
                let (event_rx, config_slot) = oneshot_channel(
                    r#"{"type":"tick","text":""}"#,
                    r#"{"config.js.tick_reply":"acked"}"#,
                );
                install_cafe(
                    &ctx,
                    &client,
                    "sess-1",
                    "js-demo",
                    event_rx,
                    config_slot,
                    Duration::from_secs(1),
                )
                .unwrap();
                ctx.eval::<(), _>(JS_PRELUDE).unwrap();
                let promise: Promise = ctx
                    .eval("(async () => { const c = await cafe.config(); return c['config.js.tick_reply']; })()")
                    .unwrap();
                promise.into_future::<String>().await.unwrap()
            })
            .await;
        assert_eq!(out, "acked");
    }

    fn runtime_chunk(annotations: &[(&str, serde_json::Value)]) -> Chunk {
        let mut chunk = Chunk::new_null("test");
        chunk = chunk.with_annotation(keys::CONFIG_TYPE, "runtime");
        for (k, v) in annotations {
            chunk = chunk.with_annotation(*k, v.clone());
        }
        chunk
    }

    #[test]
    fn resolve_config_merges_runtime_chunks_later_wins() {
        let history = vec![
            runtime_chunk(&[
                ("config.llm.model", "a".into()),
                ("config.tts.enabled", true.into()),
            ]),
            Chunk::new_text("ignored", "test"),
            runtime_chunk(&[("config.llm.model", "b".into())]),
        ];
        let cfg = resolve_config(&history);
        assert_eq!(cfg["config.llm.model"], "b");
        assert_eq!(cfg["config.tts.enabled"], true);
        assert!(cfg.get("config.type").is_none());
    }

    #[test]
    fn resolve_config_empty_without_runtime_chunks() {
        assert_eq!(
            resolve_config(&[Chunk::new_text("hi", "test")]),
            serde_json::json!({})
        );
    }

    #[test]
    fn envelopes_are_valid_json_with_ok_flag() {
        let ok: serde_json::Value =
            serde_json::from_str(&ok_envelope(serde_json::json!({"a": 1}))).unwrap();
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["result"]["a"], 1);
        let err: serde_json::Value =
            serde_json::from_str(&err_envelope(Some(-32001), "timed out")).unwrap();
        assert_eq!(err["ok"], false);
        assert_eq!(err["code"], -32001);
    }
}
