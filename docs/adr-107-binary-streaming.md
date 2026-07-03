# ADR-107: Binary streaming via binary-ref chunks

**Status**: Design complete, not yet implemented

**Context**: Binary assets (audio, images, video) are currently published as full `ContentType::Binary` chunks with base64-encoded data. This requires the entire asset to be generated before publishing, bloats bus messages, and prevents streaming playback. TTS audio, progressive images, and long video clips can't start playing until the file is fully generated and transferred.

**Decision**: A new `ContentType::BinaryRef` chunk type that announces a binary asset (id, mime_type, optional byte_size) without containing the data. A dedicated HTTP service (`cafe-binary-store`) handles the actual bytes via streaming HTTP endpoints:

- `POST /api/binary/{id}?token=<write_jwt>[&offset=N]` — stream write with optional resume
- `GET /api/binary/{id}?token=<read_jwt>` — stream read with Range support
- `DELETE /api/binary/{id}` — remove

The binary-store connects to the bus via `subscribe_filtered` for `BinaryRef` chunks. On arrival, it DM's the producer write credentials via `direct_to` mutation (ADR-102). On first byte, it publishes read credentials via broadcast mutation (ADR-103). Read JWTs never expire; write JWTs expire after configurable TTL.

The existing SSE `binary-ref` serialization (`binary_ref.rs`) is unified: both old `Binary` + `?binaryRefs=1` and real `BinaryRef` chunks produce the same SSE shape. Consumers don't need to distinguish.

GC is built into binary-store: periodic cleanup of transient files older than 30 days, session-deletion cascade for non-transient files, startup cleanup of stale `.writing` markers.

**Consequences**:
- Producers can stream bytes as they're generated (TTS plays before synthesis finishes)
- Multi-GB files supported via `?offset=N` resume (no multipart complexity)
- Bus never carries binary bytes — metadata only
- Read JWTs never expire — permanent access to session media
- Write JWTs expire — stale tokens don't accumulate
- Binary-store is stateless for reads (JWT verification only)
- New binary to operate and monitor
- Browser `<audio>`/`<img>` tags work natively with token in query string
- CORS needed for cross-origin media
- HTTPS recommended for token-in-URL security
