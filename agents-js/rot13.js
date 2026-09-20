// rot13 — ROT13 cipher: repeats all text with ROT13 rotation.
//
// JS port of agents/rot13.toml. cafe-rot13 publishes the rotated assistant
// text chunk itself, so the agent only dispatches the invoke.

const manifest = {
  name: "rot13",
  description: "ROT13 cipher: repeats all text with ROT13 rotation",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      await cafe.invoke("rot13", { text: event.text });
    }
  }
  return "rot13: done";
}
