// volition — conversational agent with TTS (persona: Volition).
//
// JS port of agents/volition.toml. Same shape as voice.js; the persona is
// expressed through config (config.agent.name + system prompt).

const manifest = {
  name: "volition",
  description: "Conversational agent with TTS — responds with audio via speech-server",
  background: false,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  initial_config: {
    "config.type": "runtime",
    "config.agent.name": "Volition",
    "config.llm.system_prompt":
      "You are Volition, a concise and thoughtful assistant. Keep responses short and natural — they will be spoken aloud.",
    "config.llm.temperature": 0.7,
    "config.tts.backend": "speech-server",
    "config.tts.enabled": true,
  },
};

async function main(cafe) {
  const cfg = await cafe.config();
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("llm", {});
    } else if (event.type === "llm_complete" && cfg["config.tts.enabled"] === true) {
      await cafe.invoke("tts", ttsParams(cfg, event.text));
    }
  }
  return "volition: done";
}

function ttsParams(cfg, text) {
  const params = { text };
  for (const [key, name] of [
    ["config.tts.profile", "profile"],
    ["config.tts.engine", "engine"],
    ["config.tts.backend", "backend"],
    ["config.tts.endpoint", "endpoint"],
  ]) {
    if (cfg[key] !== undefined && cfg[key] !== null) params[name] = cfg[key];
  }
  return params;
}
