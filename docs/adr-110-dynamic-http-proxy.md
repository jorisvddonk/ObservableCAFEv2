# ADR-110: Dynamic HTTP Proxy over Bus

**Status**: Implemented

## Context

Several bus services need HTTP endpoints:
- cafe-web-fetch needs `POST /api/sessions/:id/web` for URL fetching
- Future services (cafe-comfy, cafe-tts, cafe-stt, etc.) may want to expose HTTP interfaces

Previously, all HTTP routes were hardcoded in cafe-server's router (`router.rs`). Adding a new HTTP endpoint required modifying cafe-server's source code, recompiling, and redeploying. This worked for built-in cafe-server functionality but created a tight coupling between the HTTP gateway and individual bus services.

## Decision

Introduce a **dynamic HTTP proxy layer** over the bus. Bus services can register HTTP route patterns at runtime, and cafe-server proxies matching requests to them via private (direct_to) RPCs.

### Architecture

A well-known bus session `cafe-server.http-proxy` serves as the communication channel. All participants subscribe to it.

```
Client ──POST /api/ext/sessions/abc/fetch──▶ cafe-server
  │                                             │
  │   RouteRegistry: match path + method         │
  │   Validate body size limit                   │
  │   Publish direct_to RPC                      │
  │   Await oneshot response                     │
  │                                             │
  │          ┌──── bus (direct_to) ────┐        │
  │          │ http.request.handle RPC │         │
  │          └────────────────────────┘         │
  │                      │                      │
  │                 cafe-web-fetch               │
  │              (handles RPC, fetches URL)      │
  │                      │                      │
  │          ┌──── bus (direct_to) ────┐        │
  │          │   RPC response          │         │
  │          └────────────────────────┘         │
  │                      │                      │
  │   Receive response via oneshot               │
  │   Return HTTP {status, headers, body}        │
  ◀─────────────────────────────────────────────┘
```

### Route Registration

Services publish a transient chunk on `cafe-server.http-proxy`:

```json
{
  "annotations": {
    "cafe.http.route.register": "{\"pattern\":\"/api/ext/sessions/:id/fetch\",\"methods\":[\"POST\"]}"
  }
}
```

The bus auto-injects `source.connection` so cafe-server knows the caller's connection ID for subsequent `direct_to` RPCs. Routes are re-registered every 30s as a heartbeat.

### Route Registry (cafe-server)

`cafe-server/src/route_registry.rs` — in-memory `HashMap<String, RouteEntry>` behind `RwLock`:

- `RouteEntry { pattern, methods, connection_id, last_seen }`
- `match_path(path, method)` — iterates routes, matches with `:param` segments
- GC task runs every `CAFE_HTTP_PROXY_GC_INTERVAL_SECS` (default 30s), purges entries with `last_seen > CAFE_HTTP_PROXY_STALE_PURGE_SECS` (default 60s)

### RPC Protocol

Both request and response use `direct_to` annotations so only the addressed participant can read them.

**Request** (`http.request.handle`):
```json
{
  "jsonrpc": "2.0",
  "method": "http.request.handle",
  "params": {
    "method": "POST",
    "path": "/api/ext/sessions/abc/fetch",
    "headers": { "content-type": "application/json" },
    "query": {},
    "body": "<base64>",
    "auth": { "user_id": "admin", "token_type": "admin" }
  }
}
```

**Response**:
```json
{
  "jsonrpc": "2.0",
  "result": {
    "status": 200,
    "headers": { "content-type": "application/json" },
    "body": "<base64>"
  }
}
```

### Auth

cafe-server validates the bearer token using `AuthUser` (same as all other endpoints). The extracted auth info (`user_id`, `token_type`) is passed in the RPC params. Bus services trust what cafe-server sends — they do not re-validate.

### Body Size

Configurable via `CAFE_HTTP_PROXY_MAX_BODY_SIZE` (bytes, default 1MB). Checked at ingress in the proxy handler. Returns 413 if exceeded.

### Route Prefix

All dynamic routes live under `/api/ext/*path` to avoid conflicts with built-in routes.

### Unregistration

- **Grace period**: routes persist for `CAFE_HTTP_PROXY_STALE_PURGE_SECS` (default 60s) after the last heartbeat
- **GC**: background task runs every `CAFE_HTTP_PROXY_GC_INTERVAL_SECS` (default 30s)
- **Heartbeat**: services re-register every 30s

### Crate: `cafe-http-proxy-sdk`

A shared library (`cafe-http-proxy-sdk/`) used by both cafe-server and bus services:

- Protocol types: `RouteRegistration`, `ProxyRequest`, `ProxyAuthInfo`, `ProxyResponse`
- Helper functions: `publish_registration`, `parse_registration`, `publish_request`, `parse_request`, `publish_response`, `parse_response`
- Body encoding: `encode_body` (bytes → base64), `decode_body` (base64 → bytes)

## What moved

| Before | After |
|---|---|
| `POST /api/sessions/:id/web` hardcoded in cafe-server | Handled by cafe-web-fetch via dynamic route `/api/ext/sessions/:id/fetch` |
| `fetch_web` handler in `chunks.rs` | Removed — logic lives in cafe-web-fetch |
| No route registry | `RouteRegistry` in cafe-server |
| No proxy mechanism | `cafe-http-proxy-sdk` + `_cafe_http_proxy` session |

## Configuration

| Env Var | Default | Description |
|---|---|---|
| `CAFE_HTTP_PROXY_MAX_BODY_SIZE` | 1048576 | Max request body in bytes |
| `CAFE_HTTP_PROXY_GC_INTERVAL_SECS` | 30 | GC task interval |
| `CAFE_HTTP_PROXY_STALE_PURGE_SECS` | 60 | Route stale timeout |

## Consequences

- Bus services own their HTTP interfaces without modifying cafe-server
- Single entry point for clients (cafe-server on port 4000)
- Private RPC communication via `direct_to` — other bus participants cannot snoop
- GC removes dead service routes within 60s
- Small overhead: bus round-trip + base64 encoding per proxied request
- Body size limit prevents memory exhaustion from large payloads
