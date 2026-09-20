// stt — speech-to-text: transcribes audio using speech-server.
//
// JS port of agents/stt.toml. Transcription is agent-driven: `cafe-stt` no
// longer auto-transcribes uploaded binary_refs, so this invoke is the single
// transcription for the session. The `stt` evaluator scans session history
// for the user's audio binary_ref and publishes the transcript as assistant
// text.

const manifest = {
  name: "stt",
  description: "Speech-to-text: transcribes audio using speech-server",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("stt", {});
    }
  }
  return "stt: done";
}
