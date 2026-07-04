# ADR-109: Web fetch service

**Status**: Implemented (`e6be9c1`) — cafe-web-fetch binary, agent TOML, process-compose entry. HTTP endpoint at `POST /api/sessions/:id/web` kept as-is.

**Context**: `fetch_web` lives in `cafe-server/src/handlers/chunks.rs:62-130` as a synchronous HTTP handler. It accepts a URL, fetches it with `reqwest`, and publishes the result as a chunk with `web.source_url`, `web.content_type`, `web.fetch_time`, and optionally `web.error` annotations.

This couples content fetching to the HTTP gateway. The endpoint cannot participate in agent pipelines — there's no way to configure a `!fetch` command that triggers a web fetch through the normal pipeline flow.

**Decision (deferred)**: Keep the HTTP endpoint at `POST /api/sessions/:id/web` for direct client use. Do not move it into a standalone service at this time. The annotation keys (`web.*`) remain unprefixed data annotations.

**Future direction**: If `!fetch` functionality is desired in agent pipelines, a new step type `web-fetch` would be created (same pattern as `dice-detector` in cafe-dice):
1. Step fires on `user_message`
2. Dispatches `web-fetch.invoke` RPC with the message text
3. A `cafe-web-fetch` service handles the RPC, parses `!fetch <url>`, fetches the URL, and publishes the result chunk
4. Agents add `[[steps]] id = "web-fetch" type = "web-fetch" trigger = "user_message"` and `!fetch` naturally routes through the pipeline

For now, the HTTP endpoint remains the single path for server-side URL fetching.

**Consequences**:
- No new binary to maintain
- `POST /api/sessions/:id/web` continues to work for direct HTTP clients
- `web.*` annotation keys stay unprefixed (they're data annotations, same as `chat.*`, `config.*`)
- Agent pipelines cannot invoke web fetching as a step (deferred)
- Future migration would extract the handler into a standalone `cafe-web-fetch` binary and add a pipeline step
