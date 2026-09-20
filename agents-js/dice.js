// dice — dice-rolling test agent: `!roll 2d6`.
//
// JS port of agents/dice.toml (dice-detector -> tool-executor). The detector
// RPC parses the text and returns { detected, count, sides }; this executes
// the roll via cafe.tool, which publishes the bus-visible result.
//
// For the LLM-driven variant see dice-llm.js.

const manifest = {
  name: "dice",
  description: "Dice-rolling test agent — !roll 1d5 dispatches tool call via two-step pipeline",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type !== "user_message") continue;
    const res = await cafe.invoke("dice-detector", { text: event.text });
    if (res && res.detected) {
      await cafe.tool("dice.roll", { count: res.count, sides: res.sides });
    }
  }
  return "dice: done";
}
