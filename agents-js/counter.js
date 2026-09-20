// counter — stateful demo: JS-local state survives across events.
//
// One QuickJS runtime lives as long as the session. `n` accumulates in
// memory (no bus round-trip, no history scan); the runtime restarts —
// resetting `n` — when the file changes (hot-reload) or the session ends.

const manifest = {
  name: "counter",
  description: "Stateful demo: counts user messages in JS-local state",
  background: false,
  allows_reload: true,
  persists_state: false,
  mode: "stateful",
};

async function main(cafe) {
  let n = 0;
  for await (const event of cafe.events()) {
    if (event.type === "user_message") {
      n += 1;
      await cafe.publishText("count=" + n);
    }
  }
  return "counter: done (n=" + n + ")";
}
