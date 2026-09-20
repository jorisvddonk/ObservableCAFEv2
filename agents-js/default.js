// default — standard chat agent.
//
// JS port of agents/default.toml. The TOML pipeline was `trust-filter` +
// `llm` (trust-filter is a no-op in the legacy runtime), so this reduces to
// one `llm.invoke` per user message.

const manifest = {
  name: "default",
  description: "Standard chat agent",
  background: false,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      // The `llm` evaluator reads history and config from this session and
      // publishes the streamed reply itself — no publishText here.
      await cafe.invoke("llm", {});
    }
  }
  return "default: done";
}
