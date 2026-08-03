# Tutorial 2: Your first agent

In [Tutorial 1](getting-started.md) you ran the stack and used the built-in
`default` agent. Now you'll write your own agent — no Rust required. An agent in
ObservableCAFE is just a TOML file that describes a pipeline of steps.

By the end of this tutorial you'll have an agent that ROT13-shifts every message
sent to it.

---

## What you need

The stack from [Tutorial 1](getting-started.md) should be running (the bus and
`cafe-agent-runtime` in particular).

## Step 1 — Understand the agent file format

Agents live in `agents/*.toml`. Here's the smallest useful one, the built-in
`rot13` agent:

```toml
name = "rot13"
description = "ROT13 cipher: repeats all text with ROT13 rotation"
background = false
allows_reload = true
persists_state = false

[[steps]]
id = "rot13"
type = "rot13"
trigger = "user_message"
```

Every agent has:

- `name` — unique identifier; becomes the session ID for background agents
- `description` — what it's for
- `background` — whether it auto-starts at boot
- `allows_reload` — whether hot-reload picks up changes
- `persists_state` — whether history is written to SQLite
- `[[steps]]` — the pipeline, a list of evaluators

Each **step** has:

- `id` — a name you choose
- `type` — the evaluator type (`llm`, `tts`, `rot13`, `web-fetch`, `rss-fetch`, …)
- `trigger` — which event fires it (`user_message`, `llm_complete`, `step_complete:<id>`, …)

## Step 2 — Create your agent

Create `agents/hello.toml` with a step that echoes a friendly greeting instead of
running an LLM. We'll reuse the `rot13` evaluator — it's a tiny built-in that
proves the pipeline runs without needing a model:

```toml
name = "hello"
description = "My first agent: ROT13-shifts every message"
background = false
allows_reload = true
persists_state = false

[[steps]]
id = "cipher"
type = "rot13"
trigger = "user_message"
```

Save the file. That's it — you've written an agent.

## Step 3 — Let hot-reload pick it up

`cafe-agent-runtime` watches the `agents/` directory. Check it noticed your file:

```sh
cafe-cli list-agents
```

You should see `hello` in the list. If not, force a reload:

```sh
curl -X POST -H "Authorization: Bearer <token>" http://localhost:4000/api/admin/agents/reload
```

## Step 4 — Use your agent

Create a session with it and send a message:

```sh
SESSION=$(cafe-cli create-session --agent hello)
echo "$SESSION"

cafe-cli chat "$SESSION" "Hello world"
```

Instead of an LLM response, the pipeline runs `rot13` over your text, so the
reply is `Uryyb jbeyq` — your message shifted by 13 letters.

If `chat` prints nothing (the ROT13 evaluator may not stream), read the history:

```sh
cafe-cli history "$SESSION"
```

## Step 5 — Try a text-transform agent that doesn't need the LLM

A slightly more realistic non-LLM agent is the built-in `fetch` agent, which
downloads and displays web content:

```toml
name = "fetch"
description = "Web fetch agent — !fetch <url> to download and display web content"
background = false
allows_reload = true
persists_state = false

[[steps]]
id = "web-fetch"
type = "web-fetch"
trigger = "user_message"
```

Publish `!fetch https://example.com` to a session and you'll get the page's text
back as untrusted content (see [the trust model](../spec-cafe.md)).

## What just happened?

- `cafe-agent-runtime` loaded your TOML and registered the `hello` agent.
- When you created a session, the runtime built a pipeline from your steps.
- Each published chunk matched a `trigger`, fired the step, and the step's output
  was published back to the session.

You wrote a working agent in four lines of TOML.

## Next steps

- [Tutorial 3: A tool-calling agent](tool-calling-agent.md) — add an LLM and let it call a tool.
- Full reference for every field: [Agent config reference](../reference/agent-config.md).
- How agents run: [Agent pipelines in the architecture](../architecture.md#key-concepts).
