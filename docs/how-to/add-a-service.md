# How to: Add a service

This guide shows you how to add a brand-new bus service to ObservableCAFE —
following the pattern used by `cafe-rot13`, the smallest service in the repo. You
can clone it as your starting point.

---

## The shape of a bus service

A bus service is a standalone binary that:

1. Connects to the bus (`BusClient::unix` or via iroh).
2. Announces an **evaluator schema** so `cafe-agent-runtime` can wire it into
   agent pipelines.
3. Subscribes to sessions and handles RPC invokes (`<name>.invoke` JSON-RPC
   requests published as chunks).
4. Publishes results back as chunks.
5. Reconnects forever if the bus restarts (`run_with_reconnect`).

## Step 1 — Scaffold the crate

Add it to the workspace in `Cargo.toml`:

```toml
members = [
    ...,
    "cafe-my-service",
]
```

Create `cafe-my-service/Cargo.toml`:

```toml
[package]
name = "cafe-my-service"
version = "0.1.0"
edition = "2021"

[dependencies]
cafe-sdk = { path = "../cafe-sdk", features = ["bus-client"] }
cafe-types = { path = "../cafe-types" }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
```

## Step 2 — Main: connect, announce, subscribe

Model `src/main.rs` on `cafe-rot13`:

```rust
use anyhow::Result;
use cafe_sdk::{keys, Chunk, EvaluatorSchema, JsonRpcResponse, ServerMessage};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    cafe_sdk::bus::run_with_reconnect("cafe-my-service", move || {
        let sp = socket_path.clone();
        async move { subscribe_all(&sp).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(socket_path: &str) -> Result<()> {
    let client = cafe_sdk::bus::BusClient::unix(socket_path);

    // Announce the evaluator schema — this is what makes agent pipelines able
    // to declare a step with `type = "my-service"`.
    let schema = EvaluatorSchema {
        name: "my-service".into(),
        description: "Does something useful".into(),
        config_schema: serde_json::json!({"type": "object", "properties": {}}),
        rpc_params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Input text" }
            }
        }),
    };
    if let Err(e) = cafe_sdk::schema::announce_schema(&client, schema).await {
        warn!("cafe-my-service: failed to announce schema: {}", e);
    }

    let mut rx = client.subscribe_all().await?;
    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c).await {
                    warn!("cafe-my-service: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}
```

## Step 3 — Handle invokes

```rust
async fn run_session(session_id: String, client: cafe_sdk::bus::BusClient) -> Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if request.method != "my-service.invoke" {
            continue;
        }
        let call_id = request.id.clone();

        let text = request.params["text"].as_str().unwrap_or("");
        let result = do_something(text);

        let response = JsonRpcResponse::ok(&call_id, serde_json::json!({ "text": result }));
        let resp_chunk = Chunk::new_null("com.nominal.cafe-my-service")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;

        let text_chunk = Chunk::new_text(&result, "com.nominal.cafe-my-service")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
        let _ = client.publish(&session_id, text_chunk).await;
    }
    Ok(())
}
```

## Step 4 — Write unit tests

Unit tests go inline in `src/` under `#[cfg(test)]`, per repo convention:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn do_something_works() {
        assert_eq!(do_something("hi"), "expected");
    }
}
```

## Step 5 — Add it to the stack

Add a process entry to `process-compose.yml`, gated on the bus being healthy:

```yaml
cafe-my-service:
  command: ./target/release/cafe-my-service
  environment:
    - "RUST_BACKTRACE=1"
  readiness_probe:
    exec:
      command: "true"
    initial_delay_seconds: 1
    period_seconds: 1
    failure_threshold: 10
  depends_on:
    cafe-bus:
      condition: process_healthy
```

If the service needs a real backend (a model, a device), add `disabled: false`
with a comment explaining the requirement, or `disabled: true` if it's optional.

## Step 6 — Expose it via MCP (optional)

To make your service's tool callable by AI assistants, register it in
`cafe-mcp-bridge` following the pattern in [ADR-112](../adr-112-mcp-bridge.md).
The tool name should match the RPC method, e.g. `my_service_do_something` →
`my-service.do_something`.

## Step 7 — Wire it into an agent

Now any agent can use it:

```toml
[[steps]]
id = "my-step"
type = "my-service"
trigger = "user_message"
```

And LLM tools can reference it via `tools.available` with
`tool_type = "rpc"` (see [Tutorial 3](../tutorials/tool-calling-agent.md)).

## Step 8 — CI

Add the crate to the workspace build (automatic — it's a workspace member). If
you add an E2E test script, add it to the **`Run E2E bus tests`** step in
`.github/workflows/ci.yml` (see [Write and run E2E tests](write-e2e-tests.md)).

---

## Reference

- [ADR-121: Evaluator Schema System](../adr-121-evaluator-schema-system.md) — why `announce_schema` instead of hardcoded matches
- [`cafe.jsonrpc.*` annotations](../cafe-annotations.md#json-rpc-over-bus)
- [Data model spec: evaluators](../spec-cafe.md#3-evaluator)
- [How to: run the stack](run-the-stack.md)
