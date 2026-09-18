# ADR-126: Chunk long speech-server TTS requests

**Status**: Implemented

**Date**: 2026-09-18

**Driver**: the agent's spoken replies intermittently failed with
`POST /speak returned error status` (surfaced as proxy 502s and
`RPC step 'tts'` pipeline errors), while short utterances always worked.

**Context**: `cafe-tts` sends `POST /speak` to speech-server (`:6905`) via the
lemonade proxy (`:6900`), defaulting to `model: cosyvoice-3`. Proxy debug
endpoints (`/__proxy/status`, `/__debug/last` — see `llms.txt`) made the
failure visible without guessing.

**Decision**:

1. **Bisected the crash threshold against the live server**: 314 and 377
   chars → 200; 410 and 440 chars → server process dies (connection closed,
   proxy returns 502 `Remote end closed connection without response`, then a
   supervisor restarts the server in ~10s). Length kills it, not content
   (plain repeated text crashes identically to the same prose).
2. **Sentence-chunk in `cafe-tts`** (`speech_server.rs`): texts over
   `MAX_CHUNK_CHARS = 300` are split on sentence boundaries (overlong single
   sentences hard-split at word boundaries), each chunk synthesized with an
   individual `/speak` call, and the PCM payloads concatenated into a single
   WAV (shared `fmt ` payload, rebuilt sizes; non-PCM or mismatched formats
   bail out). Short texts take the exact same single-call path as before.
3. **Fresh connection per `/speak` request** (no pooled `reqwest::Client`):
   the proxy closes client-facing keep-alive connections after proxying a
   response, so a reused pooled connection fails the next sequential chunk
   POST with a reset. Reproduced with back-to-back POSTs on one connection
   (direct `:6905` reuse works; via-proxy reuse resets). Localhost setup cost
   is negligible next to synthesis latency.
4. **Full error chains in worker logs** (`{e}` → `{e:#}`) so the next
   diagnosis starts with the root cause, not the outer context.

**Consequences**:

- Long replies now speak as one audio chunk; per-chunk latency adds up
  sequentially (no parallel fan-out — keeps server load identical to today).
- Chunking is prompt-invisible: nothing changes in session history or the
  agent pipeline.
- The ~390-char server crash itself is upstream (brew `speech` + cosyvoice-3)
  and still exists; we just never tickle it (300-char ceiling with margin).
- `tts-speech-server-e2e.py` gained a long-text phase asserting multi-call
  splitting, per-chunk budgets, and concatenated output size.

Related: [ADR-122](adr-122-per-session-tts-backend.md) (per-session TTS
backends), [Agent config reference](reference/agent-config.md).
