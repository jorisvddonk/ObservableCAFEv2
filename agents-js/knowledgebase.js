// knowledgebase — RAG agent: searches indexed documents and answers
// questions using vector search.
//
// Unlike the (broken) agents/knowledgebase.toml, which dispatched an
// unhandled `knowledgebase-search.invoke`, this calls the method the service
// actually exposes: `knowledgebase.search`. Retrieved passages are published
// as an assistant text chunk so the follow-up `llm` turn reads them from
// history, then the LLM answers using that context.
//
// Namespace/k are agent config, overridable per session via a runtime config
// chunk (config.knowledgebase.namespace / .k).

const manifest = {
  name: "knowledgebase",
  description: "RAG agent: searches indexed documents and answers questions using vector search",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
  initial_config: {
    "config.type": "runtime",
    "config.knowledgebase.namespace": "default",
    "config.knowledgebase.k": 5,
  },
};

async function main(cafe) {
  const cfg = await cafe.config();
  const namespace = cfg["config.knowledgebase.namespace"] || "default";
  const k = cfg["config.knowledgebase.k"] || 5;

  for await (const event of cafe.events()) {
    if (event.type !== "user_message") continue;

    let context = "";
    try {
      const res = await cafe.rpc("knowledgebase.search", {
        namespace,
        query: event.text,
        k,
      });
      const results = (res && res.results) || [];
      context = results
        .map((r, i) => "[" + (i + 1) + "] " + r.text)
        .join("\n\n");
    } catch (e) {
      await cafe.log("knowledgebase search failed: " + e.message);
      context = "";
    }

    await cafe.publishText(
      context
        ? "Retrieved context:\n\n" + context
        : "No relevant documents were found."
    );
    await cafe.invoke("llm", {});
  }
  return "knowledgebase: done";
}
