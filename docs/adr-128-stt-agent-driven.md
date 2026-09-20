# ADR-128: STT is agent-driven (no auto-transcription)

**Status**: Implemented

**Date**: 2026-09-20

**Driver**: `cafe-stt` transcribed uploaded audio twice for `stt`-agent
sessions: it auto-transcribed every user `binary_ref` after its upload
completed (ADR-114), *and* answered the agent's `stt.invoke` — which also
scans history for the same `binary_ref`. Two transcripts per upload. The
auto path also made transcription implicit: any session, agent or not, would
transcribe uploaded audio with no way to opt out.

**Context**: ADR-114 introduced the `cafe.binary.completed` signal so
`cafe-stt` could transcribe a `binary_ref` once its bytes were fully written.
At the time, agents were TOML pipelines whose `stt` step was declarative, so
the completion signal doubled as the trigger. With JS agents
([ADR-127](./adr-127-js-agent-runtime.md)) the agent itself decides when to
transcribe and can `await cafe.invoke("stt", {})`, making the auto path
redundant.

**Decision**: `cafe-stt` performs transcription **only** in response to
`stt.invoke`. The auto-transcription path (tracking `binary_ref` chunks,
waiting for `cafe.binary.completed`, publishing a transcript) is removed.
`agents-js/stt.js` supplies the trigger for `stt`-agent sessions; the
`stt.invoke` handler keeps its history-scan fallback (params-less invoke) and
its `cafe.stt.base_url` / `response_format` config.

**Consequences**:

- Positive: exactly one transcript per upload for agent sessions.
- Positive: transcription is explicit and per-agent; a session only
  transcribes when its agent asks.
- Positive: `cafe-stt` no longer needs the `pending` binary-ref tracking or
  the completion-event branch — less state, fewer paths.
- Negative: sessions whose agent does not invoke `stt` no longer transcribe
  audio automatically. This is intended, but it is a behaviour change for any
  existing audio workflow that relied on the implicit path.
- Neutral: upload completion (`cafe.binary.completed`, ADR-114) is still
  published and still useful for other consumers and for debugging.

**Alternatives considered**:
- **Keep auto-transcription, drop the agent's invoke**: rejected — the user
  wants transcription driven by the `stt` agent, and it leaves no way to opt
  out for non-`stt` sessions.
- **Deduplicate instead (skip `stt.invoke` when a transcript already exists
  for that `binary_ref`)**: rejected — keeps two triggers and adds
  cross-chunk bookkeeping for a behaviour we do not want.

Related: [ADR-114](./adr-114-binary-upload-completion-event.md) (upload
completion event; its auto-transcription consequence is superseded here),
[ADR-127](./adr-127-js-agent-runtime.md) (JS agents),
[ADR-123](./adr-123-session-forking.md) (history gating).
