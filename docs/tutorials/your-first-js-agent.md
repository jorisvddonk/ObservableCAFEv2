# Tutorial: Your first JS agent

In [Tutorial 2](your-first-agent.md) you built a TOML agent. Now you'll write
the same thing as a **JS agent** — a plain script with an `async function
main(cafe)` instead of a `[[steps]]` pipeline. JS agents are the current
direction (TOML pipelines are frozen); the concepts transfer directly.

By the end you'll have a `hello-js` agent that ROT13-shifts every message,
using a real bus RPC awaited as a Promise.

---

## What you need

The stack from [Tutorial 1](getting-started.md) should be running — the bus,
`cafe-rot13`, and `cafe-agent-js` in particular.

## Step 1 — Understand the file format

JS agents live in `agents-js/*.js`. Here's the smallest useful one:

```js
const manifest = {
  name: "hello-js",
  description: "My first JS agent: ROT13-shifts every message",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      // rot13 publishes the reply itself — the agent just orchestrates.
      await cafe.invoke("rot13", { text: event.text });
    }
  }
  return "hello-js: done";
}
```

Every agent has:

- `manifest` — metadata: `name` (unique id), `description`, `background`
  (auto-start at boot), `allows_reload`, `persists_state`, `mode`
  (`stateless` or `stateful`).
- `main(cafe)` — the whole pipeline. `cafe.events()` yields
  `{ type, text }` events and `cafe.invoke("rot13", …)` awaits the evaluator's
  RPC response as a Promise.
- **`rot13` publishes the reply chunk itself**, so the agent does not call
  `cafe.publishText` here. Use `cafe.publishText` only for text the agent
  composes (see `agents-js/heartbeat.js` and `agents-js/knowledgebase.js`);
  republishing an evaluator's reply duplicates it.

Top-level code must be side-effect free — the host evaluates the file to find
`manifest` and `main`. Full field list:
[JS agent reference](../reference/js-agents.md).

## Step 2 — Create your agent

Save the snippet above as `agents-js/hello-js.js`. That's it — you've written
a JS agent.

## Step 3 — Let hot-reload pick it up

`cafe-agent-js` watches the `agents-js/` directory. Check it noticed:

```sh
cafe-cli list-agents
```

You should see `hello-js` with `source: js`. (Invalid files are skipped with
a warning in the service log — never silently.)

## Step 4 — Use your agent

Create a session with it and send a message:

```sh
SESSION=$(cafe-cli create-session --agent hello-js)
echo "$SESSION"

# `chat` goes through cafe-server; set TOKEN to your admin token (Tutorial 1)
cafe-cli --token "$TOKEN" chat "$SESSION" "Hello world"
```

The reply is `Uryyb jbeyq` — your message shifted by 13 letters, via this
path: user chunk → `user_message` event → `await cafe.invoke("rot13", …)`
(transient request, response matched by `call_id`) → `publishText`.

If `chat` prints nothing, read the history:

```sh
cafe-cli history "$SESSION"
```

## Step 5 — Change it live

Edit `agents-js/hello-js.js` — for example, change what is sent to the
evaluator:

```js
await cafe.invoke("rot13", { text: event.text.toUpperCase() });
```

Save. The host logs `hot-reloaded agent 'hello-js'` and the *same session*
uses the new code on the next message — no restart, no re-attach. (Stateful
agents restart their runtime on reload instead; their JS-local state resets.)

## What just happened?

- `cafe-agent-js` parsed your `manifest`, registered `hello-js`, and attached
  a pipeline to each of its sessions.
- Each bus chunk was classified into an event; your `for await` loop owned
  the sequencing — no trigger strings, no `step_complete` chaining.
- The RPC round-trip was one `await`, with timeouts and errors surfacing as
  rejections you can `try/catch`.

You wrote a working agent with promises and no config format.

## Next steps

- [Tutorial: a JS tool-calling agent](js-tool-calling-agent.md) — let the LLM
  decide, call a tool via RPC, and feed the result back.
- Full contract: [JS agent reference](../reference/js-agents.md).
- Stuck? [How to: debug a JS agent](../how-to/debug-js-agent.md).
