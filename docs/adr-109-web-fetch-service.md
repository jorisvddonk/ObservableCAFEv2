# ADR-109: Web fetch service

**Status**: Superseded by ADR-110 (dynamic HTTP proxy)

## History

The original `fetch_web` HTTP endpoint (`POST /api/sessions/:id/web`) in cafe-server
has been removed and replaced by a dynamic route registered by cafe-web-fetch
over the bus.

See [ADR-110](adr-110-dynamic-http-proxy.md) for the current architecture:

- `POST /api/ext/sessions/:id/fetch` → handled by cafe-web-fetch via bus proxy
- cafe-web-fetch subscribes to `cafe-server.http-proxy` session, registers route
  `/api/ext/sessions/:id/fetch`, handles `http.request.handle` RPCs
- `!fetch <url>` pipeline flow unchanged (still works via agent TOML)

## What changed

| Before | After |
|---|---|
| `POST /api/sessions/:id/web` hardcoded in cafe-server | Dynamic route `/api/ext/sessions/:id/fetch` registered by cafe-web-fetch |
| `fetch_web` handler in `chunks.rs` | Removed — logic moved to cafe-web-fetch |
| Direct HTTP response (202 Accepted) | Proxied RPC with 200/chunk_id response |
