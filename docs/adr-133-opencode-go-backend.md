# ADR-133: OpenCode Go backend for cafe-llm (direct API-key access)

**Status**: Implemented

**Date**: 2026-10-01

**Context**: cafe-llm could talk to Ollama and to a generic OpenAI-compatible
endpoint (`OpenAI_URL`). OpenCode Go is a subscription service that serves a
curated catalog of coding models — but only through its own gateway
(`https://opencode.ai/zen/go`), which has requirements a plain OpenAI-compatible
client does not satisfy:

1. **Three wire protocols, selected by model.** Chat Completions
   (`/v1/chat/completions`) for DeepSeek, GLM, Kimi, MiMo, LongCat and Hy;
   OpenAI Responses (`/v1/responses`) for Grok, GPT Luna and Muse Spark; and
   Anthropic Messages (`/v1/messages`) for MiniMax and Qwen.
2. **A stable per-conversation session header.** The gateway requires
   `x-opencode-session` so it can optimize routing and prompt caching. Requests
   without it are rejected (`MissingSessionID`).
3. **Inconsistent auth.** Chat Completions and Responses authenticate with
   `Authorization: Bearer`, but the Anthropic-compatible `/v1/messages` endpoint
   only accepts `x-api-key` (plus `anthropic-version`), and rejects the bearer
   token.
4. **A client-identifying User-Agent.** Generic SDK/HTTP-library names are
   discouraged; the client must identify itself.

The goal is direct HTTP access with an API key — never shelling out to the
`opencode` application.

**Decision**:

1. **New `OpenCodeGoBackend`** (`cafe-llm/src/backends/opencode_go.rs`) behind
   the existing `LlmBackend` trait, selected with `LLM_BACKEND=opencode-go`
   (or `opencode_go`).
2. **Model-driven protocol routing** via `protocol_for_model`: `grok*`, `gpt-*`
   and `muse-spark*` → Responses; `minimax*` and `qwen*` → Messages; everything
   else → Chat Completions. Unknown models default to Chat Completions, which
   serves the largest part of the catalog.
3. **Correct auth and required headers per protocol**: Bearer for Chat
   Completions/Responses, `x-api-key` + `anthropic-version: 2023-06-01` for
   Messages, and `x-opencode-session` + a `cafe-llm/<version>` User-Agent on
   every completion request. The session header is the cafe session id.
4. **Session id threaded through `LlmParams`.** A new optional `session_id`
   field is populated by the evaluator (for `llm.invoke`, `llm.prompt`, and
   compaction summarization) so backends that scope routing/caching to a
   conversation can use it; Ollama/OpenAI ignore it. This avoids changing the
   `LlmBackend` trait signature.
5. **Protocol-specific request shaping.** Responses moves `system` messages
   into the top-level `instructions` field and uses `max_output_tokens`;
   Messages moves `system` to the top-level `system` field, requires
   `max_tokens` (defaults to 4096) and clamps `temperature` to `[0, 1]`.
6. **Stream parsing per protocol.** Chat Completions reads
   `choices[0].delta.content`; Responses reads `response.output_text.delta`
   (ignoring reasoning events); Messages reads `content_block_delta` with
   `delta.type == "text_delta"` (ignoring thinking deltas). Error events are
   surfaced as stream errors rather than silently dropped.
7. **Config**: `OPENCODE_GO_URL` (default `https://opencode.ai/zen/go`),
   `OPENCODE_API_KEY` (empty by default), `OPENCODE_GO_MODEL` (default
   `deepseek-v4.1-flash`). Model listing uses `GET /v1/models` with Bearer auth.

**Consequences**:

- cafe-llm can use the full OpenCode Go catalog directly over HTTP with an API
  key, with no dependency on the OpenCode app or CLI.
- `LlmParams` gains a field; all constructors must set it (defaults exist via
  `Option`).
- Model routing is prefix-based and therefore needs updating when OpenCode Go
  adds a model family on a non-default protocol. Unknown models fall back to
  Chat Completions rather than failing at startup.
- `/v1/models` returns the whole catalog regardless of protocol, so a model the
  backend routes incorrectly would fail per-request rather than at list time.
- Covered by unit tests (routing, request shaping, stream extraction, error
  events) and an E2E test (`tests/opencode-go-e2e.py`) that drives all three
  protocols against a mock gateway, asserting route, auth, session header and
  streamed output.

**Alternatives considered**:

- *Reuse `OpenAiBackend` as-is.* Rejected: it cannot send `x-opencode-session`,
  a custom User-Agent, `x-api-key` auth, or speak Responses/Anthropic.
- *Shell out to the `opencode` CLI.* Rejected: the requirement is direct API
  access with an API key, and it would couple cafe-llm to an external binary.
- *Support only Chat Completions.* Rejected: the catalog's Grok, GPT Luna,
  Muse Spark, MiniMax and Qwen models would be unusable.
- *A generic multi-protocol client shared by all backends.* Deferred: would be a
  larger refactor of the existing OpenAI/Ollama backends for little immediate
  benefit.

**See also**: [ADR-134](./adr-134-per-session-llm-backend.md) makes the backend
selectable per session via `config.llm.backend`.
