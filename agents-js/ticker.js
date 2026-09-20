// ticker — scheduled JS agent (fires every 10 seconds).
//
// Cron schedules are delivered as `tick` events on the background session.
// Seven-field cron (seconds first) is accepted; five-field works too.

const manifest = {
  name: "ticker",
  description: "Scheduled JS agent: publishes a line every 10 seconds",
  background: true,
  allows_reload: true,
  persists_state: true,
  mode: "stateless",
  schedule: "*/10 * * * * * *",
};

async function main(cafe) {
  for await (const event of cafe.events()) {
    if (event.type === "tick") {
      await cafe.publishText("ticker tick");
    }
  }
  return "ticker: done";
}
