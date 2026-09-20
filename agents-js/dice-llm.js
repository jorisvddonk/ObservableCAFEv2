// dice-llm — dice rolling via LLM-generated tool calls, orchestrated in JS.
//
// The JS port of agents/dice-llm.toml: four TOML steps
// (llm → tool-detector → tool-executor → llm) become one `main`.
// The LLM emits `<|tool_call|>` markers; JS parses them, awaits each tool
// via `cafe.tool` (which publishes the bus-visible result for the
// follow-up LLM turn), then invokes the LLM again for the final answer.

const manifest = {
  name: "dice-llm",
  description: "Dice rolling via LLM-generated tool calls — full round-trip with LLM feedback",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
  initial_config: {
    "config.type": "runtime",
    "config.llm.system_prompt": [
      "You are a helpful assistant with access to a dice-rolling tool.",
      "",
      "To roll dice, you MUST invoke the dice.roll tool by emitting a tool call marker in your response, like this:",
      "",
      '<|tool_call|>{"name":"dice.roll","parameters":{"count":2,"sides":6}}<|tool_call_end|>',
      "",
      "After the tool result comes back to you, read it and tell the user what they rolled. The result will be formatted as:",
      "Tool call completed: dice.roll",
      "```",
      '{ "result": 7 }',
      "```",
      "",
      'You should then say something like "You rolled a total of 7!" in a natural way.',
      "",
      'The user may ask something like "roll 2d6" or "give me a d20". Do not mention the tool call mechanism to the user, just use it and report the result.',
    ].join("\n"),
    "tools.available": [
      {
        name: "dice.roll",
        description: "Roll one or more dice and return the total",
        parameters: {
          type: "object",
          properties: {
            count: { type: "integer", description: "Number of dice" },
            sides: { type: "integer", description: "Number of sides per die" },
          },
          required: ["count", "sides"],
        },
        tool_type: "rpc",
      },
    ],
  },
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("llm", {});
    } else if (event.type === "llm_complete") {
      let called = false;
      for (const call of findToolCalls(event.text)) {
        called = true;
        await cafe.tool(call.name, call.parameters || {});
      }
      if (called) {
        await cafe.invoke("llm", {});
      }
    }
  }
  return "dice-llm: done";
}

function findToolCalls(text) {
  const calls = [];
  const re = /<\|tool_call\|>\s*(\{.*?\})\s*<\|tool_call_end\|>/g;
  let m;
  while ((m = re.exec(text)) !== null) {
    try {
      calls.push(JSON.parse(m[1]));
    } catch {
      // Skip malformed markers; the LLM sees no result for them.
    }
  }
  return calls;
}
