# Tutorials

Guided lessons that take you from zero to running agents. Follow them in order
the first time; each builds on the previous one.

| Tutorial | You'll learn |
|---|---|
| [1. Getting started](getting-started.md) | Build the workspace, start the stack, send your first message end to end. |
| [2. Your first agent](your-first-agent.md) | Write a text-transforming agent — no Rust required. |
| [3. A tool-calling agent](tool-calling-agent.md) | Wire the LLM to a tool and back, the full round-trip (TOML). |
| [4. Your first JS agent](your-first-js-agent.md) | The same idea as (2), as a pure-JS agent: `manifest` + `async function main(cafe)`. |
| [5. A JS tool-calling agent](js-tool-calling-agent.md) | The round-trip from (3) orchestrated in promises instead of TOML steps. |

JS agents are the current direction; the TOML tutorials (2 and 3) are kept for
the frozen `cafe-agent-runtime` and for comparison.

When something doesn't work, see [How to: debug a JS
agent](../how-to/debug-js-agent.md), then the
[Reference](../reference/) for exact field names.
