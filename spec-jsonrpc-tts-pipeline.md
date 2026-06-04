# Spec: JSON-RPC Bus Protocol, Pipeline Orchestration, and cafe-tts

## Overview

This document specifies three coordinated changes:

1. **`cafe-types`** — new annotation keys and structs for JSON-RPC over the bus
2. **`cafe-agent-runtime`** — pipeline executor that orchestrates steps via JSON-RPC
3. **`cafe-tts`** — new binary that handles `tts.*` JSON-RPC method calls

These changes implement the pattern where `cafe-agent-runtime` owns pipeline
sequencing and explicitly dispatches work to specialised service binaries via
JSON-RPC requests published as null chunks on the session bus. Service binaries
are optional — if they are not running, their pipeline steps time out gracefully.

---

## Part 1: cafe-types additions

### New annotation key constants (`annotation.rs`)

Add to the `keys` module:

```rust
// JSON-RPC over bus
pub const JSONRPC_REQUEST:  &str = "jsonrpc.request";
pub const JSONRPC_RESPONSE: &str = "jsonrpc.response";
```

### New structs (`jsonrpc.rs` — new file)

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request, carried in annotation "jsonrpc.request".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,          // always "2.0"
    pub id: String,               // UUID v4; used for response correlation
    pub method: String,           // e.g. "tts.speak", "stt.transcribe"
    pub params: Value,            // method-specific parameters object
}

/// A JSON-RPC 2.0 response, carried in annotation "jsonrpc.response".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,          // always "2.0"
    pub id: String,               // matches the request id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: uuid::Uuid::new_v4().to_string(),
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    pub fn ok(id: impl Into<String>, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id: id.into(), result: Some(result), error: None }
    }

    pub fn err(id: impl Into<String>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }

    pub fn is_ok(&self) -> bool { self.error.is_none() }
}
```

### Standard JSON-RPC error codes

```rust
pub mod rpc_errors {
    pub const PARSE_ERROR:      i32 = -32700;
    pub const INVALID_REQUEST:  i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS:   i32 = -32602;
    pub const INTERNAL_ERROR:   i32 = -32603;
    // Application-level errors (-32000 to -32099)
    pub const SERVICE_UNAVAILABLE: i32 = -32000;
    pub const TIMEOUT:              i32 = -32001;
    pub const UPSTREAM_ERROR:       i32 = -32002;
}
```

### Helper on `Chunk`

Add to `chunk.rs`:

```rust
impl Chunk {
    /// If this chunk carries a JSON-RPC request, return it.
    pub fn as_rpc_request(&self) -> Option<JsonRpcRequest> {
        self.get_annotation(keys::JSONRPC_REQUEST)
    }

    /// If this chunk carries a JSON-RPC response, return it.
    pub fn as_rpc_response(&self) -> Option<JsonRpcResponse> {
        self.get_annotation(keys::JSONRPC_RESPONSE)
    }

    /// True if this is a JSON-RPC response matching the given call id.
    pub fn is_rpc_response_for(&self, call_id: &str) -> bool {
        self.as_rpc_response()
            .map(|r| r.id == call_id)
            .unwrap_or(false)
    }
}
```

### Wire format examples

**RPC request chunk** (published by cafe-agent-runtime to session output stream):
```json
{
  "id": "aaaaaaaa-...",
  "content_type": "null",
  "producer": "com.nominal.cafe-agent-runtime",
  "annotations": {
    "jsonrpc.request": {
      "jsonrpc": "2.0",
      "id": "bbbbbbbb-...",
      "method": "tts.speak",
      "params": {
        "text": "The full assembled assistant response text.",
        "profile": "Volition",
        "engine": "qwen"
      }
    }
  },
  "timestamp": 1717123456789
}
```

**RPC response chunk** (published by cafe-tts to the same session):
```json
{
  "id": "cccccccc-...",
  "content_type": "null",
  "producer": "com.nominal.cafe-tts",
  "annotations": {
    "jsonrpc.response": {
      "jsonrpc": "2.0",
      "id": "bbbbbbbb-...",
      "result": {
        "chunk_id": "dddddddd-..."
      }
    }
  },
  "timestamp": 1717123456900
}
```

The audio binary chunk (`dddddddd-...`) is published by `cafe-tts` to the session
*before* the response chunk, so it is already in history when the runtime receives
the response.

---

## Part 2: cafe-agent-runtime — pipeline executor

### Concept

The pipeline executor is the core new piece of `cafe-agent-runtime`. It:

1. Knows the pipeline steps for a session (from the agent TOML)
2. Triggers on input chunks (user messages, scheduler ticks, STT results)
3. Walks the pipeline steps in order
4. For built-in steps (`role-annotator`, `trust-filter`): executes them in-process
5. For RPC steps (`tts`, `stt`, `rss-fetch`): publishes a JSON-RPC request chunk
   and awaits a matching response chunk on the session stream
6. On timeout or error response: publishes an error chunk and stops the pipeline

### Step types

```rust
/// A single pipeline step as parsed from agent TOML.
#[derive(Debug, Clone)]
pub enum PipelineStep {
    /// Handled in-process by cafe-agent-runtime.
    BuiltIn(BuiltInEvaluator),
    /// Dispatched via JSON-RPC to an external service binary.
    Rpc(String),  // method namespace, e.g. "tts", "stt"
}

#[derive(Debug, Clone)]
pub enum BuiltInEvaluator {
    RoleAnnotator,
    TrustFilter,
    // future: others that are truly lightweight and have no external deps
}
```

TOML step name → step type mapping:

| TOML name        | Type                       | Method namespace |
|------------------|----------------------------|-----------------|
| `role-annotator` | BuiltIn(RoleAnnotator)     | —               |
| `trust-filter`   | BuiltIn(TrustFilter)       | —               |
| `tts`            | Rpc("tts")                 | `tts.*`         |
| `stt`            | Rpc("stt")                 | `stt.*`         |
| `llm`            | Rpc("llm")                 | `llm.*`         |

Note: `llm` becomes an RPC step too, dispatched to `cafe-llm`. This is the right
long-term shape — `cafe-agent-runtime` doesn't call the LLM directly, it requests
that `cafe-llm` handle the turn. `cafe-llm` already does this implicitly by
subscribing to sessions; the RPC pattern makes it explicit and awaitable.

> **Scope note for this implementation:** Wire up `tts` as the first RPC step.
> The `llm` RPC transition can be a follow-up — `cafe-llm` can continue its
> current behaviour (subscribe + auto-respond) while `tts` is built as the
> first explicit RPC worker. The two approaches coexist safely because LLM
> response chunks have `chat.role: assistant` and stream_complete markers that
> the runtime already waits for.

### Pipeline executor pseudocode

```rust
pub struct PipelineExecutor {
    steps: Vec<PipelineStep>,
    rpc_timeout: Duration,        // from config, default 30s
}

impl PipelineExecutor {
    /// Run the pipeline for one trigger chunk.
    /// `publish` sends a chunk to the session output stream.
    /// `await_rpc_response` waits for a response chunk with matching id.
    pub async fn run(
        &self,
        trigger: &Chunk,
        history: &[Chunk],
        session_id: &str,
        bus: &BusClient,
    ) -> Result<(), PipelineError> {

        // State passed between steps
        let mut context = PipelineContext {
            current_chunk: trigger.clone(),
            session_id: session_id.to_string(),
            history,
        };

        for step in &self.steps {
            match step {
                PipelineStep::BuiltIn(evaluator) => {
                    // In-process, synchronous
                    context.current_chunk = evaluator.process(&context)?;
                }

                PipelineStep::Rpc(namespace) => {
                    let config = resolve_session_config(history);
                    let params = build_rpc_params(namespace, &context, &config);
                    let method = format!("{}.invoke", namespace);

                    let request = JsonRpcRequest::new(&method, params);
                    let call_id = request.id.clone();

                    // Publish the RPC request as a null chunk
                    let req_chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
                        .with_annotation(keys::JSONRPC_REQUEST, &request);
                    bus.publish(session_id, req_chunk).await?;

                    // Await matching response on this session's stream
                    let response = bus
                        .await_chunk(session_id, |c| c.is_rpc_response_for(&call_id))
                        .timeout(self.rpc_timeout)
                        .await
                        .map_err(|_| PipelineError::Timeout {
                            step: namespace.clone(),
                            call_id: call_id.clone(),
                        })??;

                    let rpc_resp = response.as_rpc_response().unwrap();
                    if !rpc_resp.is_ok() {
                        return Err(PipelineError::RpcError {
                            step: namespace.clone(),
                            error: rpc_resp.error.unwrap(),
                        });
                    }

                    // Update context with result for next step
                    context.last_rpc_result = rpc_resp.result;
                }
            }
        }
        Ok(())
    }
}
```

### RPC params construction per namespace

```rust
fn build_rpc_params(
    namespace: &str,
    ctx: &PipelineContext,
    config: &SessionConfig,
) -> serde_json::Value {
    match namespace {
        "tts" => json!({
            "text": ctx.assembled_assistant_text(),  // collected from streaming history
            "profile": config.tts_profile,
            "engine": config.tts_engine,
        }),
        "stt" => json!({
            "chunk_id": ctx.current_chunk.id,
            "base_url": config.stt_base_url,
        }),
        _ => json!({}),
    }
}
```

### When does the TTS step trigger?

The pipeline runs once per *user turn completion*. The executor watches the session
input stream for user chunks (role = user), then walks the pipeline. The LLM step
fires (implicitly via cafe-llm's existing subscription for now), and the executor
waits for `chat.stream_complete` before proceeding to the `tts` step.

The executor assembles the full assistant text by scanning session history for
chunks with `chat.role: assistant` and `chat.is_streaming: true` that arrived
after the most recent user chunk, concatenated in order.

---

## Part 3: cafe-tts

### New binary in the workspace

Add to `Cargo.toml` workspace members: `"cafe-tts"`

```
cafe-tts/src/
├── main.rs          # connect to bus, run worker loop
├── worker.rs        # subscribe_all, dispatch RPC requests
├── voicebox.rs      # Voicebox HTTP client
└── config.rs        # Config from env
```

### What it does

1. Connects to cafe-bus with `subscribe_all`
2. For every incoming chunk on any session:
   - Ignores chunks that are not null chunks with `jsonrpc.request`
   - Ignores requests where `method` is not in the `tts.*` namespace
   - For `tts.invoke`: extracts params, calls Voicebox, publishes audio chunk,
     publishes JSON-RPC response
3. Handles errors by publishing a JSON-RPC error response

### Voicebox client (`voicebox.rs`)

Uses the `/speak` endpoint — takes profile by name, which is what the config
stores. This avoids needing to manage profile UUIDs.

```rust
pub struct VoiceboxClient {
    base_url: String,   // default http://127.0.0.1:17493
    http: reqwest::Client,
}

impl VoiceboxClient {
    /// POST /speak — returns raw audio bytes (WAV or MP3 depending on Voicebox config)
    pub async fn speak(
        &self,
        text: &str,
        profile: &str,
        engine: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {  // (bytes, mime_type)
        let body = serde_json::json!({
            "text": text,
            "profile": profile,
        });
        // engine is passed as a query param if present, per Voicebox API
        let url = match engine {
            Some(e) => format!("{}/speak?engine={}", self.base_url, e),
            None => format!("{}/speak", self.base_url),
        };

        let response = self.http
            .post(&url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let mime = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("audio/wav")
            .to_string();

        let bytes = response.bytes().await?.to_vec();
        Ok((bytes, mime))
    }
}
```

### Worker loop (`worker.rs`)

```rust
pub async fn run_worker(bus: BusClient, voicebox: VoiceboxClient) {
    bus.subscribe_all().await.unwrap();

    loop {
        let (session_id, chunk) = bus.next_chunk().await.unwrap();

        // Only handle null chunks with a JSON-RPC request
        let Some(request) = chunk.as_rpc_request() else { continue };
        if !request.method.starts_with("tts.") { continue };

        let call_id = request.id.clone();

        let result = handle_tts_request(&voicebox, &request, &session_id, &bus).await;

        // Publish response (success or error)
        let response = match result {
            Ok(audio_chunk_id) => JsonRpcResponse::ok(
                &call_id,
                json!({ "chunk_id": audio_chunk_id }),
            ),
            Err(e) => JsonRpcResponse::err(
                &call_id,
                rpc_errors::UPSTREAM_ERROR,
                e.to_string(),
            ),
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-tts")
            .with_annotation(keys::JSONRPC_RESPONSE, &response);
        bus.publish(&session_id, resp_chunk).await.ok();
    }
}

async fn handle_tts_request(
    voicebox: &VoiceboxClient,
    request: &JsonRpcRequest,
    session_id: &str,
    bus: &BusClient,
) -> anyhow::Result<String> {
    let text    = request.params["text"].as_str().unwrap_or_default();
    let profile = request.params["profile"].as_str().unwrap_or("default");
    let engine  = request.params["engine"].as_str();

    let (audio_bytes, mime_type) = voicebox.speak(text, profile, engine).await?;

    // Publish audio as a binary chunk BEFORE the response
    let audio_chunk = Chunk::new_binary(audio_bytes, mime_type, "com.nominal.cafe-tts")
        .with_annotation(keys::CHAT_ROLE, "assistant");

    let audio_chunk_id = audio_chunk.id.clone();
    bus.publish(session_id, audio_chunk).await?;

    Ok(audio_chunk_id)
}
```

### Cargo.toml dependencies

```toml
[dependencies]
cafe-types  = { path = "../cafe-types" }
tokio       = { workspace = true }
serde       = { workspace = true }
serde_json  = { workspace = true }
tracing     = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow      = { workspace = true }
reqwest     = { version = "0.12", features = ["json"] }
```

Note the minimal dependency footprint — no audio processing libraries needed.
Voicebox handles all audio encoding. `cafe-tts` is just an HTTP bridge.

### Environment variables

| Variable               | Default                    | Description              |
|------------------------|----------------------------|--------------------------|
| `CAFE_BUS_SOCKET`      | `/tmp/cafe-bus.sock`       | Bus socket               |
| `VOICEBOX_URL`         | `http://127.0.0.1:17493`   | Voicebox base URL        |

### process-compose.yml addition

```yaml
cafe-tts:
  command: ./target/debug/cafe-tts
  depends_on:
    cafe-bus:
      condition: process_healthy
  environment:
    - "VOICEBOX_URL=http://127.0.0.1:17493"
  restart_policy:
    restart: on_failure
  # Optional — only start if Voicebox is running
  disabled: false
```

---

## Summary of changes per crate

| Crate                | Change                                                        |
|----------------------|---------------------------------------------------------------|
| `cafe-types`         | Add `jsonrpc.rs`, annotation key constants, `Chunk` helpers  |
| `cafe-agent-runtime` | Add `PipelineExecutor`, `PipelineStep`, RPC dispatch + await |
| `cafe-tts`           | New binary — Voicebox bridge, JSON-RPC worker                 |
| `Cargo.toml`         | Add `cafe-tts` to workspace members                           |
| `process-compose.yml`| Add `cafe-tts` service                                        |

---

## Implementation order

1. `cafe-types` changes first — everything else depends on them
2. `cafe-tts` second — can be tested standalone against a live bus and Voicebox
   before the pipeline executor is wired up
3. `cafe-agent-runtime` pipeline executor last — builds on the types and can be
   integration-tested end-to-end with `cafe-tts` running

---

## Tests

### cafe-types
- `JsonRpcRequest::new` generates a valid UUID id and sets jsonrpc = "2.0"
- `JsonRpcResponse::ok` / `::err` serialize correctly and round-trip
- `Chunk::is_rpc_response_for` matches on id, rejects mismatched id

### cafe-tts
- Unit test `handle_tts_request` with a mock Voicebox server (use `mockito` or `wiremock`)
- Assert audio chunk is published before response chunk
- Assert response chunk carries matching `call_id`
- Assert error response is published when Voicebox returns non-200

### cafe-agent-runtime
- Unit test `PipelineExecutor::run` with a mock bus
- Assert RPC request chunk is published with correct method and params
- Assert pipeline proceeds after receiving matching response
- Assert `PipelineError::Timeout` is returned when no response arrives within timeout