# How to: Use OpenCode Go with cafe-llm

[OpenCode Go](https://opencode.ai/docs/go/) is a subscription that serves a
curated catalog of coding models over an HTTP API. `cafe-llm` can call it
directly with your API key — there is no dependency on the `opencode`
application or CLI.

## 1. Get an API key

Subscribe at [opencode.ai/auth](https://opencode.ai/auth), then copy the API key
from the console.

## 2. Provide the API key

Set the credentials for the `cafe-llm` process (see `process-compose.yml` / your
launchd environment):

```sh
OPENCODE_API_KEY=sk-...                       # required
OPENCODE_GO_URL=https://opencode.ai/zen/go    # default
OPENCODE_GO_MODEL=deepseek-v4.1-flash         # default model for this provider
```

`OPENCODE_API_KEY` is always needed. Every backend is constructed at startup, so
you do **not** need to restart to switch providers.

## 3. Select the provider (per session or process default)

Select OpenCode Go for a single session with `config.llm.backend`:

```sh
cafe-cli --bus /tmp/cafe-bus.sock publish <session> --null \
  --annotation config.type=runtime \
  --annotation config.llm.backend=opencode-go
```

If you omit `config.llm.model`, the provider's default
(`OPENCODE_GO_MODEL`) is used. Sessions that do not set
`config.llm.backend` use the process default (`LLM_BACKEND`, e.g. a local
OpenAI-compatible proxy).

To make OpenCode Go the process-wide default instead, set:

```sh
LLM_BACKEND=opencode-go
```

and start the stack as usual (`./start.sh`). cafe-llm logs the router config and
lists models from every backend via `GET /v1/models`.

## 4. Pick a model per session

Any model from the Go catalog can be selected per session with
`config.llm.model`:

```sh
cafe-cli --bus /tmp/cafe-bus.sock publish <session> --null \
  --annotation config.type=runtime \
  --annotation config.llm.model=grok-4.6
```

cafe-llm routes the model to the correct wire protocol automatically:

| Model family | Protocol | Endpoint |
|---|---|---|
| DeepSeek, GLM, Kimi, MiMo, LongCat, Hy | OpenAI Chat Completions | `/v1/chat/completions` |
| Grok, GPT Luna, Muse Spark | OpenAI Responses | `/v1/responses` |
| MiniMax, Qwen | Anthropic Messages | `/v1/messages` |

Auth differs per protocol (`Authorization: Bearer` for the first two,
`x-api-key` + `anthropic-version` for Messages); cafe-llm handles this for you.
Every completion request also carries the cafe session id in the
`x-opencode-session` header, which the gateway uses for routing and prompt
caching.

## Troubleshooting

- **`MissingSessionID`** — the gateway did not receive `x-opencode-session`.
  This should not happen with cafe-llm; if it does, the request likely bypassed
  the backend (for example, a custom proxy).
- **`AuthError: Missing API key`** — `OPENCODE_API_KEY` is unset, or the request
  hit `/v1/messages` without the `x-api-key` header.
- **Model not in the list** — `/models` in the OpenCode console reflects your
  subscription; cafe-llm lists whatever `GET /v1/models` returns.

## See also

- [ADR-133: OpenCode Go backend for cafe-llm](../adr-133-opencode-go-backend.md)
- [ADR-134: Per-session LLM backend selection](../adr-134-per-session-llm-backend.md)
- [Agent config reference](../reference/agent-config.md)
- [How to: Run the stack](run-the-stack.md)
