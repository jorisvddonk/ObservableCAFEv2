# cafe-llm — Build Guide

**Role:** LLM evaluator bridge. Subscribes to session input streams on the bus,
calls LLM backends for chunks with `chat.role: user`, streams response chunks
back to the session output stream.

**Build after:** `cafe-types`, `cafe-bus`

---

## What it does

- Connects to cafe-bus
- Is told which sessions to handle (by cafe-agent-runtime or by subscribing to all)
- When a user chunk arrives, builds conversation context from history and calls the LLM
- Streams response tokens back as chunks with `chat.role: assistant` and
  `chat.is_streaming: true`; sends a final null chunk with `chat.stream_complete: true`
- Supports multiple backends: Ollama, OpenAI-compatible (including local endpoints),
  KoboldCPP

> **v0.1 scope:** Support Ollama and OpenAI-compatible. KoboldCPP is a stretch goal.

---

## Cargo.toml dependencies to add

```toml
[dependencies]
cafe-types   = { path = "../cafe-types" }
tokio        = { workspace = true }
serde        = { workspace = true }
serde_json   = { workspace = true }
tracing      = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow       = { workspace = true }
reqwest      = { version = "0.12", features = ["json", "stream"] }
futures-util = "0.3"
async-trait  = "0.1"
```

---

## File structure

```
cafe-llm/src/
├── main.rs             # connect to bus, dispatch to handlers
├── backends/
│   ├── mod.rs          # LlmBackend trait
│   ├── ollama.rs       # Ollama streaming API
│   └── openai.rs       # OpenAI-compatible streaming API
├── context.rs          # build conversation context from chunk history
├── evaluator.rs        # main evaluation loop: history → LLM → chunks
├── bus_client.rs       # bus connection + subscription management
└── config.rs           # Config from env vars
```

---

## LlmBackend trait

```rust
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Stream response chunks for a given conversation context.
    /// Yields text chunks; caller wraps them with annotations.
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        params: &LlmParams,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<String>>>;
}

pub struct LlmMessage {
    pub role: String,    // "user", "assistant", "system"
    pub content: String,
}

pub struct LlmParams {
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}
```

---

## Ollama backend

Endpoint: `POST $OLLAMA_URL/api/chat`

```json
{
  "model": "gemma3:1b",
  "messages": [
    { "role": "system", "content": "You are a helpful assistant." },
    { "role": "user", "content": "Hello" }
  ],
  "stream": true
}
```

Ollama streams NDJSON where each line is:
```json
{ "message": { "role": "assistant", "content": "Hello" }, "done": false }
```

Use `reqwest` with `.bytes_stream()` and `futures_util::StreamExt` to iterate lines.

---

## OpenAI-compatible backend

Endpoint: `POST $OPENAI_URL/v1/chat/completions`

Uses standard SSE `data:` lines with `[DONE]` terminator.
Set `Authorization: Bearer $OPENAI_API_KEY` header.

---

## Context building (context.rs)

```rust
pub fn build_messages(history: &[Chunk], system_prompt: Option<&str>) -> Vec<LlmMessage> {
    let mut messages = Vec::new();

    // 1. Add system prompt if present
    if let Some(prompt) = system_prompt {
        messages.push(LlmMessage { role: "system".into(), content: prompt.into() });
    }

    // 2. Walk history, include only:
    //    - Text chunks with chat.role = user or assistant
    //    - Text chunks that are trusted (no security.trust-level or trusted = true)
    for chunk in history {
        if chunk.content_type != ContentType::Text { continue; }
        let trust = chunk.get_annotation::<serde_json::Value>("security.trust-level");
        if let Some(t) = trust {
            if t["trusted"] == false { continue; }
        }
        match chunk.role() {
            Some("user") | Some("assistant") => {
                messages.push(LlmMessage {
                    role: chunk.role().unwrap().into(),
                    content: chunk.content.clone().unwrap_or_default(),
                });
            }
            _ => {}
        }
    }

    messages
}
```

---

## Evaluation loop (evaluator.rs)

When a user chunk arrives on `input_stream`:

1. Extract current config from session history (scan for most recent `config.type: runtime` null chunk)
2. Call `build_messages(history, system_prompt)`
3. Create a streaming request to the LLM backend
4. For each token: publish a text chunk with:
   - `content`: the token text
   - `chat.role`: `assistant`
   - `chat.is_streaming`: `true`
   - `chat.model`: the model name
5. After the stream ends, publish a null chunk with:
   - `chat.stream_complete`: `true`
   - `chat.finish_reason`: `stop` (or from the API response)

Errors: publish to the error stream (null chunk with `error.message` annotation),
do not crash the evaluation loop.

---

## Abort handling

When a null chunk with `flow.signal: abort` arrives on the session input stream,
cancel any in-flight LLM request for that session. Use a `tokio::sync::watch`
channel per active session to signal abort.

---

## Environment variables

| Variable         | Default                    | Description              |
|------------------|----------------------------|--------------------------|
| `CAFE_BUS_SOCKET`| `/tmp/cafe-bus.sock`       | Bus socket path          |
| `LLM_BACKEND`    | `ollama`                   | `ollama` or `openai`     |
| `OLLAMA_URL`     | `http://localhost:11434`   | Ollama base URL          |
| `OLLAMA_MODEL`   | `gemma3:1b`                | Default model            |
| `OPENAI_URL`     | `http://localhost:8000`    | OpenAI-compat base URL   |
| `OPENAI_API_KEY` | *(empty)*                  | API key                  |
| `OPENAI_MODEL`   | *(empty)*                  | Model name               |
