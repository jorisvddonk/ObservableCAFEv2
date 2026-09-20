// voice — conversational agent with TTS.
//
// JS port of agents/voice.toml: user message -> llm, then on the LLM's
// completion, speak the reply via `tts.invoke`. TTS is gated on
// `config.tts.enabled` (the TOML `enabled_if`), read fresh per event.
//
// `cafe-tts` publishes the audio BinaryRef itself; the agent only dispatches.

const manifest = {
  name: "voice",
  description: "Voice agent with TTS via speech-server",
  background: false,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  initial_config: {
    "config.type": "runtime",
    "config.llm.system_prompt":
      "You are a concise voice assistant. Keep responses brief and natural — they will be spoken aloud.",
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
  return "voice: done";
}

// Mirror the legacy pipeline's tts params: text plus any session config.
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
