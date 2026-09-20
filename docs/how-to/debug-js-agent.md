# How to: Debug a JS agent

This guide covers the failure modes of JS agents (`agents-js/*.js` run by
`cafe-agent-js`) and how to diagnose each one.

---

## The agent doesn't appear in `list-agents`

```sh
cafe-cli list-agents
```

JS agents show `source: js`. If yours is missing, the manifest failed to load —
check the service log for `skipping …`:

- `must define a top-level manifest object` — the file has no `manifest` global.
- `manifest.name must be a non-empty string` / bad `mode` — fix the fields
  (see [JS agent reference](../reference/js-agents.md#manifest-fields)).
- Top-level side effects — manifest extraction evaluates the file; keep the
  top level to `manifest` + `main` definitions.

## The agent runs but answers nothing

Pull the session history and look for the error chunk:

```sh
cafe-cli history "$SESSION"
```

Failures publish a null chunk with `error.source: js-agent` and
`cafe.error.message` carrying the JS stack, e.g.:

```json
{ "error.source": "js-agent", "cafe.error.message": "js agent failed: ReferenceError: foo is not defined\n    at main ..." }
```

Common causes:

- **RPC rejection** — the awaited evaluator errored or timed out
  (`manifest.rpc_timeout_secs`, default 30 s). Check the target service is
  running (e.g. `cafe-rot13` for `rot13.invoke`).
- **Wrong result shape** — `cafe.invoke("rot13", …)` resolves with the
  evaluator's `result` object (`{ text: … }`), not a string. Log it with
  `cafe.log(JSON.stringify(res))` and read the service log.
- **Event never arrives** — only `user_message`, `llm_complete`, and `tick`
  wake the agent; transient RPC chunks don't. A publisher that sends before
  the host attaches can also be missed (history replay doesn't trigger).

## The reply looks wrong after an edit

Hot-reload applies to subsequently read sources: stateless agents pick it up
on the next event; stateful agents respawn on the next event **with fresh JS
state**. Confirm the reload in the log (`hot-reloaded agent '…'`). If the
file is invalid, the old version keeps running and the log says why.
`allows_reload: false` ignores changes entirely. Schedule changes need a
restart.

## Reproduce in isolation

For a deterministic repro, run the agent against a throwaway bus with the
E2E harness pattern (temp socket, only the services you need):

```sh
cargo build -p cafe-bus -p cafe-cli -p cafe-rot13 -p cafe-agent-js
```

then start each binary with `CAFE_BUS_SOCKET` pointed at a temp socket and
`CAFE_JS_AGENT_PATHS` at a scratch dir. The committed
[`tests/js-agents-e2e.py`](../../tests/js-agents-e2e.py) is a complete
example: registry assertions, RPC `call_id` correlation, stateful counters,
ticks, cron, and both hot-reload paths.

## Checklist

1. `list-agents` shows the agent with `source: js`.
2. Service log shows `loaded N JS agents` with no `skipping …` for yours.
3. Session history has no `error.source: js-agent` chunks.
4. For RPC: the matching `{ns}.invoke` request/response pair shares a
   `call_id` in `cafe.jsonrpc.request` / `cafe.jsonrpc.response`.

## Reference

- [JS agent reference](../reference/js-agents.md)
- [ADR-127: JS Agent Runtime](../adr-127-js-agent-runtime.md)
- [Write and run E2E tests](write-e2e-tests.md)
