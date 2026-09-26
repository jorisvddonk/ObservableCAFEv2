# ADR-131: Outbound HTTP from JS agents (`cafe.fetch`)

**Status**: Implemented

**Date**: 2026-09-20

**Driver**: JS agents could only do bus-mediated I/O. Anything touching the
network needed a dedicated evaluator on the bus — `cafe-web-fetch` for pages,
`cafe-rss` for feeds — so a one-line API call meant writing and running a
service. Agents and their authors expect something like the browser's
`fetch`.

**Context**: [ADR-127](./adr-127-js-agent-runtime.md) intentionally gave the
JS context no `fs`/net/timers, with all I/O flowing through the `cafe` bridge.
That keeps agents inside the trust model, but it makes ordinary HTTP calls
disproportionately expensive. The agent code itself is trusted (it runs in the
`cafe-agent-js` host process with the host's privileges), so the sandbox was
about *shape*, not isolation from the machine.

**Decision**: Add `cafe.fetch(url, options)` to the bridge, also exposed as the
global `fetch`, implemented with `reqwest` inside the host:

- Options: `method` (default `GET`), `headers` (object or `[k, v]` pairs),
  `body` (string). Request timeout is the agent's `rpc_timeout_secs`.
- Response: `{ ok, status, statusText, url, headers, text(), json() }`, where
  `headers` is a `Headers`-like object (`get`/`has`/`forEach`/`entries`/`keys`/
  `values`).
- Semantics match the browser API: **resolve** for any HTTP status (including
  4xx/5xx, with `ok: false`) and **reject** with a `TypeError` only on
  transport errors.
- Bodies are text for now — `text()`/`json()`. No streaming, `arrayBuffer`,
  `FormData`, `AbortSignal`, or redirect-policy control yet.

Fetched content is not automatically trusted: it reaches the LLM only if the
agent publishes it as a chunk (same rule as elsewhere). Services remain the
right tool for shared, cacheable or trust-annotated fetching
(`cafe-web-fetch` stamps `security.trust-level: untrusted`; `cafe-rss` returns
structured items).

**Consequences**:

- Positive: agents can call HTTP APIs, webhooks and feeds directly, without a
  bespoke evaluator per source.
- Positive: familiar Fetch-shaped API; `await fetch(...)` works as expected.
- Negative: agent code now has the host's network egress. A prompt-injected
  agent could fetch arbitrary URLs (SSRF/exfiltration surface). This is the
  main reason the capability is documented rather than silent.
- Negative: supersedes the "no net" half of ADR-127's sandbox statement.
- Neutral: no timeout override per call (uses the agent's RPC timeout); no
  response size cap yet.

**Alternatives considered**:
- **Keep bus-only fetching**: rejected — correct for shared/untrusted sources,
  but too heavy for trivial per-agent HTTP.
- **Allowlist of hosts via config**: attractive for hardening; deferred until
  there is a concrete need.
- **Full WHATWG Fetch** (streams, AbortSignal, FormData, redirect control):
  rejected as far more surface than needed now.

Related: [ADR-127](./adr-127-js-agent-runtime.md) (JS agent runtime and its
original sandbox), [ADR-109](./adr-109-web-fetch-service.md) (web fetch as a
service), [ADR-129](./adr-129-cafe-rss-service.md) (RSS as a service).
