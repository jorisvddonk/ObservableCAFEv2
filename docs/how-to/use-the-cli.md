# How to: Use the cafe-cli

`cafe-cli` is a command-line client for the bus. It's the fastest way to poke the
system while developing, and it's what the e2e tests use under the hood.

Every command takes `--bus <path>` to point at a specific socket (default
`/tmp/cafe-bus.sock`) and `--verbose` for tracing output.

---

## Create a session

```sh
SESSION=$(cafe-cli create-session --agent default)
echo "$SESSION"
```

Omit `--agent` to use the `default` agent. You can pick your own session ID:

```sh
cafe-cli create-session my-session --agent default
```

## Publish a message

```sh
cafe-cli publish "$SESSION" --text "hello world"
```

Other payload kinds:

```sh
# Binary file (uploaded to the binary store)
cafe-cli publish "$SESSION" --file photo.png --mime image/png

# Announce a binary ref without uploading bytes
cafe-cli publish "$SESSION" --binary-ref --mime audio/wav --transient

# Null chunk carrying config annotations
cafe-cli publish "$SESSION" --null --annotation config.type=runtime \
    --annotation config.llm.system_prompt="You are a poet."

# Transient chunk (never persisted)
cafe-cli publish "$SESSION" --text "ephemeral" --transient
```

`--wait <secs>` holds the publishing connection open for mutations to arrive on
the chunk (e.g. binary write URLs injected by the binary store).

## Chat (streaming round-trip)

The one-shot convenience command: sends a message and prints the assistant's
SSE-streamed reply as JSON chunks. It goes through `cafe-server`, so it needs
the admin token (see [Getting started](../tutorials/getting-started.md)) — and
`--token` is a global option, so it comes **before** the subcommand:

```sh
cafe-cli --token "$TOKEN" chat "$SESSION" "What is the capital of France?"
```

## Subscribe

```sh
# Live stream for one session (with history replay), 5s default timeout
cafe-cli subscribe "$SESSION" --timeout-secs 10

# All sessions, optionally filtered by content type
cafe-cli subscribe-all --timeout-secs 3
cafe-cli subscribe-all --content-type text --timeout-secs 3
```

Output is newline-delimited JSON of `chunk` events. The
`{"event":"history_complete",...}` line marks the switch from history replay to
live streaming.

## Read history

```sh
cafe-cli history "$SESSION"
```

## List things

```sh
cafe-cli list-sessions   # active sessions
cafe-cli list-models     # LLM models available via cafe-llm
cafe-cli list-agents     # registered agent definitions
```

## Delete a session

```sh
cafe-cli delete-session "$SESSION"
```

## Binary-store operations

```sh
cafe-cli store --help
# e.g. upload a file and get back a read URL/token
```

## iroh / remote bus

```sh
# Connect over iroh using the bus's addr file (auto-discovery)
cafe-cli --bus /path/to/cafe-bus.sock create-session --agent default

# Explicit key + relay
cafe-cli --bus-iroh-key <pubkey> --bus-iroh-relay https://euc1-1.relay.n0.iroh.link/ \
    create-session --agent default

# Manage the iroh peer-ID allowlist
cafe-cli iroh-allowlist list
cafe-cli iroh-allowlist add <peer-id> --label "home"
cafe-cli iroh-allowlist remove <peer-id>
cafe-cli iroh-allowlist my-id
```

See [How to: connect over iroh](connect-over-iroh.md) for the full story.

---

## Reference

- [Bus protocol spec](../spec-bus-protocol.md) — the messages behind every command
- [HTTP API spec](../spec-http-api.md) — the same operations over HTTP
