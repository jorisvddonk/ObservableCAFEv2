# ADR-125: Configurable LLM history compaction

**Status**: Implemented

**Date**: 2026-09-17

**Driver**: `cafe-llm` sent the full unbounded session history on every
`llm.invoke` (`cafe-llm/src/context.rs::build_messages`), so long sessions
grew the prompt linearly until the backend context window errored. There was
no truncation, summarization, or token budgeting of any kind.

**Context**: History compaction must be per-session (different agents have
different context needs), use the existing `config.llm.*` runtime-config
channel (null chunks with `config.type = "runtime"`, later-wins per-key
merge), and remain backward compatible (sessions without the new keys behave
exactly as before).

**Decision**:

1. **New config keys** (`config.llm.*`, namespaced-only, no legacy flat
   equivalents):
   - `config.llm.compaction_mode`: `"none"` (default) | `"truncate"` |
     `"summarize"`.
   - `config.llm.max_history_messages`: max user+assistant messages sent to
     the backend (leading `system` message excluded from the count and never
     dropped).
   - `config.llm.max_history_chars`: max summed `content.len()` chars over
     user+assistant messages. Chars are a deliberate proxy for tokens (no
     per-model tokenizer dependency).
2. **Semantics** (`apply_compaction` in `cafe-llm/src/context.rs`, applied
   after `build_messages` coalescing so budgets count actual LLM messages):
   both budgets apply, whichever hits first wins; oldest dropped first; the
   newest message is never dropped (even if it alone exceeds the char budget);
   `0`/invalid/missing budgets are treated as unset; `none`/unset/invalid
   mode is identity. `CompactionInfo.dropped` carries the dropped prefix for
   summarization.
3. **`summarize` mode** (`evaluator.rs::summarize_prefix`): the dropped prefix
   is condensed via a direct non-streaming backend call
   (`build_summary_request` → `complete_to_string`) using the session's model,
   and the result is inserted into the prompt via `insert_summary` (right after
   a leading `system` message, else first). Prompt-only: nothing is written
   back to history. Empty summaries and backend errors fall back to the
   truncated list with a warning. The summary call is a direct backend call,
   not an `llm.invoke` round-trip, so it cannot recurse.
4. **Plumbing**: new `keys::CONFIG_LLM_*` constants in cafe-types; new
   `Option` fields on `SessionConfig` in `cafe-types/src/envelope.rs` (creation
   path) and `cafe-agent-runtime/src/config.rs` (merge path) plus
   `cafe-bus/src/client.rs::make_config_chunk`; new `LlmConfig` fields plus
   `extract_config` parsing in `cafe-llm`; `evaluator.rs` applies compaction
   on the `llm.invoke` path only (`llm.prompt` is stateless) and logs
   `dropped_messages/dropped_chars/mode`.
5. **Discovery**: the `llm` evaluator schema (`bus_client.rs`) and
   `cafe-web-sdk` `SessionConfig` advertise the new keys. Reference docs
   (`docs/reference/agent-config.md`, `docs/spec-cafe.md`) list them.

**Consequences**:

- Sessions opt in per-agent via `[initial_chunk.annotations]` or mid-session
  config chunks; unset keys preserve today's full-history behavior.
- Truncation is lossy by design: dropped prefix messages are invisible to the
  backend but remain in session history (no history mutation).
- Char budgets approximate token usage; very long single messages still go
  through (newest-kept rule) and can exceed backend windows — callers needing
  hard guarantees should set conservative budgets.
- `summarize` mode adds one non-streaming backend call per compacted
  `llm.invoke` (same session model); failures and empty summaries fall back
  to truncation with a warning, so generation never blocks on summarization.

**Alternatives considered**:

- *Token-count budgets via `chat.token_count`*: precise but requires a
  per-model tokenizer in the hot path; rejected in favor of the char proxy.
- *Mutating history (deleting old chunks)*: would break the event-sourced
  history contract (ADR-002); rejected — compaction is prompt-construction
  only.
- *Env-var defaults*: rejected per request — session chunks are the only
  surface, keeping behavior explicit per agent.

Related: [ADR-002](adr-002-event-sourced-chunk-model.md),
[ADR-108](adr-108-annotation-key-namespace.md),
[ADR-121](adr-121-evaluator-schema-system.md),
[Agent config reference](reference/agent-config.md).
