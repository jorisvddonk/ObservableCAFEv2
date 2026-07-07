# ADR-115: HTTP BinaryRef Publish Rejection

- **Status:** Accepted
- **Date:** 2026-07-07
- **Driver:** Binary upload write credentials are delivered via `direct_to` (ADR-102), which requires the publishing connection to stay alive. HTTP connections are ephemeral, so credentials are unreachable.

## Context

The HTTP API endpoint `POST /api/sessions/:id/chunks` accepts `content_type: "binary_ref"` and publishes the chunk to the session bus via `BusClient::publish` (fire-and-forget, fresh connection per call).

The binary-store monitors the bus for `BinaryRef` chunks, extracts `cafe.source.connection` from the chunk's annotations, and replies with write credentials via `publish_direct` targeting that connection ID.

This works for WebSocket clients because they use `SessionSubscription::publish` (persistent connection). The connection stays alive for the duration of the WebSocket session, so the `direct_to` reply reaches the client.

For HTTP clients, the publishing connection is dropped before the binary-store can reply. The bus returns `TARGET_NOT_FOUND` and the write credentials are silently lost. Read credentials (via broadcast mutation) still work.

## Decision

Reject `content_type: "binary_ref"` requests at the HTTP handler level with a clear error message directing users to the WebSocket endpoint.

No other content types are affected. BinaryRef chunks published via WebSocket continue to work.

## Consequences

- HTTP users get an immediate, actionable error instead of a silent credential loss.
- No breaking change to WebSocket clients (they publish via `ws_handler.rs`, not this endpoint).
- No impact on read credentials (delivered via broadcast mutation to all subscribers).
- No impact on existing SSE streaming (`GET /api/sessions/:id/stream`).

## Alternatives considered

1. **Store pending direct_to replies in the bus** for a short window after connection close, delivered on reconnect. Adds complexity with unclear benefit — the HTTP client has no persistent channel to receive them.

2. **Deliver write credentials via broadcast mutation** instead of `direct_to`. Security concern: anyone subscribed to the session would receive a write token, not just the original publisher.

3. **Return a polling URL** for credentials. Adds round-trips and client complexity; the credential may never arrive (if binary-store is slow or fails).
