# YouTube Data API for live checks: evaluated and rejected (2026-08)

Question: could `src/live-check/platforms/youtube.ts` switch from HTML scraping to the
official [YouTube Data API v3](https://developers.google.com/youtube/v3/getting-started)?

**Answer: no.** The API cannot support 20-second polling (`LiveCheckTask` runs
`*/20 * * * * *` = 4,320 checks/channel/day) by a factor of ~4,000×. Keep the scraper.

## Why not

1. **The only liveness endpoint is `search.list`.** Detecting whether an arbitrary
   channel is live requires `search.list?eventType=live&channelId=X`.
   - `channels.list` has no live-status field.
   - `videos.list` exposes live data (`snippet.liveBroadcastContent`,
     `liveStreamingDetails.concurrentViewers`) only for a **known video ID** — and
     discovering the live video ID is the whole problem.
   - The Live Streaming API (`liveBroadcasts.list`) only covers your *own*
     authorized channel.

2. **`search.list` is hard-capped at 100 calls/day** under the
   [current quota model](https://developers.google.com/youtube/v3/determine_quota_cost):
   "100 `search.list` calls, 100 `videos.insert` calls, and 10,000 units per day
   combined for all other endpoints." That budget checks ONE channel every ~14
   minutes. Even the old model (10,000 units/day, search at 100 units/call) would
   have been 43× over for a single channel at 20s cadence.

3. **No batching escape hatch.** `search.list` takes exactly one `channelId` per
   call. The endpoints that batch (up to 50 IDs on `videos.list`/`channels.list`,
   1 unit) are the ones that can't detect liveness. There is no
   "which of these channels is live" call anywhere in the API.

4. **Freshness lag.** `search.list` is index-backed; community reports put its
   live-detection delay at minutes (sometimes 10+), which would gut 20-second
   detection even if quota were unlimited.

5. Quota increases require the YouTube API Services compliance audit, oriented at
   production applications — not realistically granted for a personal notifier.
   (Getting a default-quota API key itself is trivial; that's not the blocker.)

## Hybrid considered and rejected

Scrape for liveness + video ID, then one batched `videos.list` call per tick for
`concurrentViewers` (~4,320 units/day regardless of channel count — fits). Rejected
because the scraper already extracts the viewer count from the same HTML
(`extractViewerCount`), so the API would add a key, a quota, and a dependency while
providing nothing new.

## If the scraper ever becomes painful

Harden the scrape instead (e.g. also parse the `/live` redirect for a video ID as a
second signal). This is why every open-source YouTube live notifier scrapes `/live`
rather than using the official API.
