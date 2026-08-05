# ADR-122: Per-session TTS backend selection

**Status**: Implemented (commit `feat(cafe-tts): per-session TTS backend switching`)

**Date**: 2026-08-05

**Context**: cafe-tts chose its backend once at startup from the `TTS_BACKEND`
env var. `SpeechServerClient::synthesize` drops the `profile` argument entirely,
and the worker only read RPC request params (`text`/`profile`/`engine`), never
session config. The advertised evaluator schema listed `config.tts.engine`
("voicebox, speech-server") and `config.tts.endpoint`, but neither selected a
backend nor overrode a URL at runtime — a per-session switch was impossible.

**Decision**:

1. **New config key `config.tts.backend`** with enum values `"voicebox"` /
   `"speech-server"`. Added `keys::CONFIG_TTS_BACKEND` in cafe-types and a
   `tts_backend: Option<String>` field on `SessionConfig`, resolved by
   `apply_config_key` like the other `config.tts.*` keys.
2. **Passed through the existing RPC-params channel**: `build_rpc_params` for
   the `tts` namespace now includes `backend` and `endpoint` alongside the
   existing `profile`/`engine`, matching how those values already flow from the
   agent runtime to the evaluator.
3. **Dual-client service**: cafe-tts constructs both `VoiceboxClient` and
   `SpeechServerClient` at startup regardless of the env default. A new
   `TtsService` owns both plus the process default backend and dispatches per
   request:
   - `config.tts.backend` selects the backend; unknown/empty values fall back
     to the process-level `TTS_BACKEND` (with a warning).
   - `config.tts.endpoint`, when set, overrides the backend's base URL for that
     request (both clients are cheap: base URL + `reqwest::Client`).
4. **Schema corrected**: the `tts` evaluator schema now declares
   `config.tts.backend` and `config.tts.endpoint`, and `config.tts.engine` is
   described as an engine name within a backend (e.g. `qwen` for voicebox)
   rather than a backend selector.

**Consequences**:

- A session can now pick its TTS backend independently of the stack default —
  e.g. an agent config sets `config.tts.backend = "voicebox"`, while the stack
  default stays `speech-server`.
- Both TTS HTTP clients are now always constructed, adding two cheap
  `reqwest::Client`s at startup.
- The `TtsClient` enum was replaced by `TtsService`; any external callers of
  that type must migrate.
- Existing sessions without `config.tts.backend` behave exactly as before
  (process default), so the change is backward compatible.

**Alternatives considered**:

- *Worker resolves session config from history itself* (like cafe-llm's
  `extract_config`): more self-contained but duplicates config resolution and
  requires the worker to fetch history on every call; rejected in favor of
  keeping the existing params-based contract.
- *Reuse `config.tts.engine` as the backend selector*: ambiguous with the
  engine-name meaning (`qwen`); rejected in favor of a distinct key.
