// heartbeat — background JS agent with config seeding.
//
// Starts at boot (background: true): the host creates the `heartbeat`
// session and publishes `initial_config` as a runtime null chunk.
// Answers user messages via rot13 and acknowledges scheduler ticks.

const manifest = {
  name: "heartbeat",
  description: "Background JS agent: config seeding + tick handling",
  background: true,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  initial_config: {
    "config.js.agent": "heartbeat",
    "config.js.tick_reply": "heartbeat tick acknowledged",
  },
};

async function main(cafe) {
  const cfg = await cafe.config();
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      const res = await cafe.invoke("rot13", { text: event.text });
      await cafe.publishText(res.text);
    } else if (event.type === "tick") {
      const reply =
        (cfg && cfg["config.js.tick_reply"]) || "tick";
      await cafe.publishText(reply);
    }
  }
  return "heartbeat: done";
}
