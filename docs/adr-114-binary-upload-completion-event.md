# ADR-114: Binary Upload Completion Event + Auto-Transcription

**Status**: Implemented

## Context

The STT pipeline for voice transcription has a race condition:

1. Client publishes a `binary_ref` chunk → pipeline fires `stt.invoke` immediately
2. Cafe-stt looks for read credentials in session history — they don't exist yet
3. Client receives write credentials (transient), starts uploading audio
4. Binary-store publishes read credentials on first byte of upload (see ADR-108 § BinaryRef)
5. Cafe-stt has already errored: "missing audio"

The read credentials mutation (step 4) is published on the **first byte** of upload, not after completion.
The file may still be uploading — the `.writing` sidecar file still exists, and the GET endpoint
returns `done=false` (see `storage.rs:134`).

## Decision

### 1. Completion Event (binary-store)

After the upload fully completes and the `.writing` sidecar file is removed, the binary-store
publishes a non-transient mutation chunk:

```rust
Chunk::mutation(chunk_id)
    .with_annotation("cafe.binary.completed", true)
```

This mutation targets the binary_ref chunk via `cafe.mutates.target_id` and signals that
the audio is fully uploaded and ready for consumption.

**Added in:** `cafe-binary-store/src/main.rs` — `publish_completion()` called after both
`finalize()` paths (single-shot write and segmented write).

### 2. Auto-Transcription (cafe-stt)

Cafe-stt's `run_session()` now maintains a `pending` map of BinaryRef chunk IDs that have
`chat.role = user`. When a completion event arrives (`cafe.binary.completed = true`) for
a tracked BinaryRef, cafe-stt:

1. Fetches read credentials from session history (published on first byte, ADR-108)
2. Downloads the complete audio from the binary-store via the read URL + JWT
3. Transcribes via voicebox
4. Publishes the transcription as an assistant text chunk (`chat.role = assistant`)

This is fully autonomous — no pipeline timing, no polling, no RPC dispatch needed.
The `stt.invoke` RPC handler is still available for direct base64 audio input.

### Sequence

```
Client                    cafe-bus            binary-store          cafe-stt
  │                          │                    │                    │
  │  binary_ref (chat.user)  │                    │                    │
  │ ──────────────────────►  │ ──────────────────► │ ─────────────────► │
  │                          │                    │  tracks ref_id     │
  │  write creds (direct_to) │                    │                    │
  │ ◄────────────────────────┤ ◄────────────────── │                    │
  │                          │                    │                    │
  │  PUT /api/binary/{id}    │                    │                    │
  │ ──────────────────────────────────────────────► │                    │
  │                          │                    │  read creds        │
  │                          │                    │  (first byte)      │
  │                          │                    │ ─────────────────► │
  │                          │                    │                    │
  │                          │                    │  completion event  │
  │                          │                    │  (done writing)    │
  │                          │                    │ ─────────────────► │
  │                          │                    │                    │
  │                          │                    │  auto-transcribe   │
  │                          │                    │ ◄──────────────────│
  │                          │                    │  downloads audio   │
  │                          │                    │ ─────────────────► │
  │                          │                    │                    │
  │                          │                    │  assistant chunk   │
  │                          │                    │ ─────────────────► │
  │                          │                    │                    │
  │  ◄── transcription ──── │ ◄────────────────── │                    │
```

## Related ADRs

| ADR | Relation |
|-----|----------|
| **ADR-108** — Binary Streaming | Defines BinaryRef content type, write/read credentials, the `.writing` sidecar pattern. This ADR extends the completion signal. |
| **ADR-113** — WebSocket Bridge | WebSocket endpoint for HTTP clients. The auto-transcription in cafe-stt completes the voice pipeline for WebSocket clients. |

## Consequences

- No race condition — transcription happens after the audio is fully uploaded
- No polling — cafe-stt responds to the completion event in real-time
- No pipeline changes — the `stt` step in agent TOMLs remains declarative
- Cafe-stt handles both paths: direct `stt.invoke` (base64 audio) and auto-transcription (binary_ref)
- The completion event is non-transient (appears in history) for audit/debugging
- Binary-store publishes 2 events per upload: read credentials (first byte) + completed (finalized)
