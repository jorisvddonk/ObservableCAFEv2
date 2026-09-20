# ADR-129: RSS as its own evaluator (cafe-rss)

**Status**: Implemented

**Date**: 2026-09-20

**Driver**: The `rss-summarizer` agent had no working implementation. Its TOML
declared `rss-fetch` and `llm-summarize` steps, but neither evaluator existed
anywhere on the bus — both dispatches timed out — and the `schedule = "0 7 * *
*"` was 5-field, which `tokio-cron-scheduler` rejects, so it never even
registered.

**Context**: Feed fetching is a distinct capability (HTTP GET + XML parsing
for RSS 2.0 and Atom) with its own failure modes. Bundling it into
`cafe-web-fetch` would mix two concerns: web-fetch's output is *untrusted*
HTML-stripped page text that the LLM deliberately skips (the trust filter),
whereas a feed digest is meant to be summarized. A separate evaluator keeps
that trust decision with the calling agent.

**Decision**: Add a dedicated thin service, `cafe-rss`, exposing
`rss-fetch.invoke`:

- Params: `url` (required; `text` accepted as fallback), `limit` (default 10).
- Returns `{ title, items: [{ title, link, published, summary }] }` in the
  RPC result — **it does not publish or summarize**. The calling agent decides
  how to present the feed and whether to involve the LLM.
- Parses both RSS 2.0 (`<item>`) and Atom (`<entry>`, `<link href>`), ignoring
  namespaces, via `quick-xml` (already in the workspace lockfile).
- Announces an `EvaluatorSchema` named `rss-fetch` (ADR-121).

`agents-js/rss-summarizer.js` drives it: on each tick it calls `rss-fetch`,
formats the items into a digest, publishes that as an assistant text chunk,
and then invokes `llm` to summarize — on a **7-field** cron
(`"0 0 7 * * * *"`, daily 07:00).

**Consequences**:

- Positive: the feed capability is reusable by any agent, and the trust
  decision (whether a feed digest reaches the LLM) stays with the agent.
- Positive: `rss-summarizer` now works end to end (previously dead on arrival),
  and its schedule finally registers.
- Positive: no new dependency — `quick-xml` was already in the lockfile.
- Negative: a feed digest published by an agent is ordinary assistant text,
  i.e. trusted for the LLM. Agents that want to keep feed content out of the
  LLM simply do not invoke `llm`.
- Negative: the parser is deliberately minimal (local names only, best-effort
  unknown entities dropped). It is not a full feed-spec implementation.

**Alternatives considered**:
- **Put `rss-fetch` in `cafe-web-fetch`**: rejected — conflates untrusted web
  page text with feed digests and blurs the trust model.
- **Parse in the JS agent**: rejected — agents have no network access.
- **Return feed text only (no items)**: rejected — structured items let the
  agent format and limit the digest without re-parsing.

Related: [ADR-109](./adr-109-web-fetch-service.md) (web fetch as a bus
service), [ADR-121](./adr-121-evaluator-schema-system.md) (evaluator schemas),
[ADR-127](./adr-127-js-agent-runtime.md) (JS agents).
