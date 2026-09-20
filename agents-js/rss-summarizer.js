// rss-summarizer — fetches and summarizes an RSS/Atom feed on a schedule.
//
// Background agent on a daily cron. On each tick it calls the cafe-rss
// `rss-fetch` evaluator (which returns parsed feed items — it does not
// publish or summarize), formats them into a digest, publishes that as an
// assistant text chunk, then asks the LLM to summarize it.
//
// Feed URL is agent config (`config.rss.url`), overridable per session with a
// runtime config chunk.
//
// Note: the cron is 7-field with seconds (tokio-cron-scheduler rejects
// 5-field expressions): "0 0 7 * * * *" = 07:00:00 daily.

const manifest = {
  name: "rss-summarizer",
  description: "Fetches and summarizes RSS feeds daily",
  background: true,
  allows_reload: true,
  persists_state: false,
  mode: "stateless",
  schedule: "0 0 7 * * * *",
  initial_config: {
    "config.type": "runtime",
    "config.rss.url": "https://news.ycombinator.com/rss",
  },
};

async function main(cafe) {
  const cfg = await cafe.config();
  const url = cfg["config.rss.url"];

  for await (const event of cafe.events()) {
    if (event.type !== "tick") continue;
    if (!url) {
      await cafe.log("rss-summarizer: no config.rss.url set; skipping tick");
      continue;
    }

    let digest;
    try {
      const feed = await cafe.invoke("rss-fetch", { url, limit: 10 });
      const items = (feed && feed.items) || [];
      if (items.length === 0) {
        await cafe.publishText("No items found in the feed.");
        continue;
      }
      digest =
        "Feed: " + (feed.title || url) + "\n\n" +
        items
          .map(function (it, i) {
            const summary = (it.summary || "").replace(/\s+/g, " ").slice(0, 200);
            return (i + 1) + ". " + it.title + (it.link ? "\n   " + it.link : "") +
              (summary ? "\n   " + summary : "");
          })
          .join("\n\n");
    } catch (e) {
      await cafe.log("rss-summarizer: fetch failed: " + e.message);
      await cafe.publishText("Could not fetch the feed: " + e.message);
      continue;
    }

    await cafe.publishText(digest);
    await cafe.invoke("llm", {});
  }
  return "rss-summarizer: done";
}
