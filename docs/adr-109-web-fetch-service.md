# ADR-109: Web fetch service

**Status**: Implemented (`fe841c7`)

## What was done

A new standalone binary `cafe-web-fetch` was created (same pattern as `cafe-dice`):

- New crate: `cafe-web-fetch/` with `Cargo.toml` + `src/main.rs`
- Handles `web-fetch.invoke` RPC dispatched by the pipeline
- Parses `!fetch <url>` from user message text
- Fetches the URL with `reqwest`, strips HTML, publishes a text chunk with `web.source_url`, `web.content_type`, `web.fetch_time`, `security.trust-level` annotations
- 4 unit tests for `strip_html`

New agent TOML at `agents/fetch.toml`:

```toml
[[steps]]
id = "web-fetch"
type = "web-fetch"
trigger = "user_message"
```

This lets any session created with `--agent fetch` handle `!fetch <url>` commands through the normal pipeline flow.

Added to `process-compose.yml` with dependency on `cafe-bus`.

## What was kept

The existing HTTP endpoint `POST /api/sessions/:id/web` in `cafe-server/src/handlers/chunks.rs` is **unchanged**. It continues to work for direct HTTP clients that want to trigger a web fetch without going through a pipeline.

## Context

`fetch_web` originally lived only in `cafe-server/src/handlers/chunks.rs:62-130` as a synchronous HTTP handler. It accepted a URL, fetched it with `reqwest`, and published the result as a chunk. This coupled content fetching to the HTTP gateway — there was no way to trigger a web fetch through the agent pipeline.

## Decision

1. Extract the fetch logic into a standalone bus-connected binary `cafe-web-fetch`
2. Create a corresponding pipeline step type `web-fetch` that dispatches `web-fetch.invoke` RPC
3. Keep the HTTP endpoint for direct client use (no reason to remove it)

## Consequences

- `!fetch <url>` works naturally in the pipeline: create a session with the `fetch` agent, send a message
- `POST /api/sessions/:id/web` still works for HTTP clients
- `web.*` annotation keys stay unprefixed (they're data annotations, same as `chat.*`, `config.*`)
- No logic duplication: the HTTP endpoint and the bus handler share the same pattern but are independently maintained
