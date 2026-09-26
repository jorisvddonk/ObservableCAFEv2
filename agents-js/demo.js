// js-demo — file-loaded echo agent (no LLM needed).
//
// A JS agent is a plain script defining two globals:
//   `manifest`   — metadata (see cafe-js-manifest/src/lib.rs)
//   `main(cafe)` — async entry point; `cafe.events()` is an async generator
//                  of `{ type, text }` events; `cafe.invoke` / `cafe.rpc`
//                  are real Promises resolved by bus RPC responses.
//
// Top-level code must be side-effect free: the host evaluates the whole file
// once per event (stateless mode) just to reach `main`.

const manifest = {
  name: "demo",
  description: "JS echo via rot13 RPC — hello world without an LLM",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      // `rot13` publishes the reply chunk itself, so the agent must not also
      // `publishText` it — that would produce a duplicate reply. Agents only
      // publish content an evaluator did not (see heartbeat's tick reply, or
      // knowledgebase's retrieved context).
      await cafe.invoke("rot13", { text: event.text });
    }
  }
  return "demo: done";
}
