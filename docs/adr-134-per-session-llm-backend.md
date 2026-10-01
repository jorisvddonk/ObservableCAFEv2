# ADR-134: Per-session LLM backend selection

**Status**: Implemented

**Date**: 2026-10-01

**Context**: [ADR-133](./adr-133-opencode-go-backend.md) added an OpenCode Go
backend, but which backend `cafe-llm` used was fixed for the whole process at
startup from `LLM_BACKEND`. The evaluator read `config.llm.backend` out of
session history (`context.rs`) but never used it, so the schema advertised a
per-session backend that had no effect. Users wanted one stack to serve, for
example, a local default model while individual sessions opted into OpenCode Go.

**Decision**:

1. **`BackendRouter`** (`cafe-llm/src/backends/router.rs`) implements
   `LlmBackend` and owns every configured backend (Ollama, OpenAI-compatible,
   OpenCode Go). `main` constructs all of them unconditionally — each is just a
   `reqwest::Client` plus a base URL — and wraps them in the router.
2. **`LlmParams.backend`** carries the per-request provider override. The
   evaluator sets it from `cfg.backend` for `llm.invoke` (and for the
   compaction-summarization call, so summarization uses the same provider as the
   generation it belongs to).
3. **Selection with a safe fallback**: the router normalizes names
   (`opencode_go` / `opencode-go`, case-insensitive), selects the requested
   backend, and on an unknown/empty name logs a warning and falls back to the
   process default (`LLM_BACKEND`). No request is rejected for a bad backend
   name.
4. **Per-backend default model**: the router exposes
   `default_model_for(backend)`. When a session selects a provider but no model,
   the evaluator uses that provider's configured default (e.g.
   `OPENCODE_GO_MODEL`), falling back to the process default model. This makes
   "select the provider only" work without also naming a model from a different
   provider.
5. **Model listing aggregates** the models of every backend, so the registry
   exposes local and OpenCode Go models together.
6. **Scope**: `llm.prompt` is a low-level primitive that takes its params from
   the RPC and does not consult session config; it continues to use the process
   default backend. The `llm` agent step (`llm.invoke`) honors
   `config.llm.backend`.

**Consequences**:

- A session selects its provider with `config.llm.backend = "opencode-go"` (and
  optionally `config.llm.model`), independent of the process default.
- All backends are always constructed; harmless for the local backends and
  required so a session can switch without a restart.
- `LlmBackend` gains a `default_model_for` default method returning `None`;
  only the router overrides it.
- Unknown backend names degrade to the process default with a warning rather
  than failing. A typo therefore selects the wrong provider silently apart from
  the warning; operators should check logs when a session uses an unexpected
  provider.
- Covered by router unit tests (routing, aliases, fallback, per-backend default
  models, aggregation) and by `tests/opencode-go-e2e.py`, which runs a stack
  whose process default is a local mock and drives sessions that select OpenCode
  Go per session (chat/responses/messages), a backend-only session, and a
  model-only session that must use the process default.

**Alternatives considered**:

- *Restart cafe-llm with a different `LLM_BACKEND` per provider.* Rejected: not
  per-session and disruptive.
- *One shared generic HTTP client with a per-session URL.* Rejected: the three
  OpenCode Go protocols already need dedicated code, and the local backends have
  distinct transports.
- *Reject requests whose `config.llm.backend` is unknown.* Rejected in favor of
  a warning + process default, matching how an unset backend behaves and
  avoiding a hard failure from stale session config.
- *Model-only selection without per-backend defaults.* Rejected: a session that
  selects OpenCode Go but not a model would otherwise send a local model name to
  the Go gateway.

**See also**: [ADR-122: Per-session TTS backend selection](./adr-122-per-session-tts-backend.md)
uses the same dual-client/service dispatch pattern for TTS.
