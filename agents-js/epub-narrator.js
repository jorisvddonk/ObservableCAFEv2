// epub-narrator — vocalizes EPUB books chapter by chapter.
//
// JS port of agents/epub-narrator.toml. Commands: `!load <path>`, `!next`,
// `!chapter <n>`, `!list`. cafe-epub answers `epub.invoke` and emits its own
// TTS requests and state annotations, so the agent only forwards the text.
//
// Long chapters synthesize slowly, hence the 300s RPC timeout.

const manifest = {
  name: "epub-narrator",
  description: "Vocalizes EPUB books chapter by chapter via TTS — !load, !next, !chapter, !list",
  background: false,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  rpc_timeout_secs: 300,
  initial_config: {
    "config.type": "runtime",
    "config.tts.backend": "speech-server",
    "config.tts.enabled": true,
  },
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("epub", { text: event.text });
    }
  }
  return "epub: done";
}
