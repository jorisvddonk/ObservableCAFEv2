// fetch — web fetch agent: `!fetch <url>` downloads and displays web content.
//
// JS port of agents/fetch.toml. `cafe-web-fetch` serves `web-fetch.invoke`
// from the agent's `text` (`!fetch <url>` or a bare URL) and publishes the
// page text as a chunk annotated untrusted web content — so it is shown to
// the user but never fed back into the LLM (see cafe-llm's trust filter).

const manifest = {
  name: "fetch",
  description: "Web fetch agent — !fetch <url> to download and display web content",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("web-fetch", { text: event.text });
    }
  }
  return "fetch: done";
}
