/**
 * End-to-end smoke test for the Castro enqueue path — no LLM, no discovery.
 *
 *   npx dotenvx run -- npx tsx src/tools/castro-smoke.ts
 *
 * Exercises the real PodcastAccountClient contract against the live account:
 * resolve a fixed unsubscribed show from its RSS feed, enqueue its latest
 * episode at Queue Next, VERIFY it actually landed in the server queue (an
 * "added" result only means the POST returned 200 — a disabled sync session
 * still 200s the projection while never broadcasting), then dequeue it and
 * verify it is gone. Leaves the real queue untouched on success.
 *
 * It cannot verify on-device rendering (that needs a phone), but a pass means
 * the credential is a live sync peer and the resolve→match→enqueue→dequeue
 * chain works.
 */
import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import {
  PodcastQueuePosition,
  resolvePodcastAccount,
} from "../podcast-recs/account.js";
import { fetchFeedEpisodes } from "../podcast-recs/rss.js";
import config from "../utils/config.js";

Injector.configure({ config });
const logger = new Logger("CastroSmoke");

// Radiolab: a show the owner does not subscribe to (Simplecast feed whose RSS
// guids differ from Castro's, so this also covers media-URL matching).
const SHOW_TITLE = "Radiolab";
const FEED_URL = "https://feeds.simplecast.com/EmVW7VGp";

function fail(message: string): never {
  logger.error(`SMOKE FAILED: ${message}`);
  process.exit(1);
}

const account = resolvePodcastAccount(logger);
if (!account) fail("no Castro account configured (CASTRO_ACCESS_ID/CASTRO_SECRET_KEY)");

const episode = (await fetchFeedEpisodes(FEED_URL, { maxEpisodes: 1 }))[0];
if (!episode) fail("could not read the test feed");
logger.info(`Test episode: ${SHOW_TITLE} — ${episode.title}`);

// 1. Enqueue via the real path.
const enqueue = await account.enqueueEpisode({
  feedUrl: FEED_URL,
  episodeGuid: episode.guid,
  mediaUrl: episode.enclosureUrl,
  showTitle: SHOW_TITLE,
  episodeTitle: episode.title,
  position: PodcastQueuePosition.Next,
});
if (enqueue !== "added" && enqueue !== "already_exists") {
  fail(`enqueueEpisode returned "${enqueue}"`);
}
logger.info(`enqueueEpisode → ${enqueue}`);

// 2. Verify it actually landed in the server queue (not just a 200).
const afterEnqueue = await account.fetchQueue();
if (afterEnqueue.status !== "ok")
  fail(`fetchQueue unavailable: ${afterEnqueue.reason}`);
const ours = afterEnqueue.value.find(
  (item) => item.showTitle === SHOW_TITLE && item.episodeTitle === episode.title,
);
if (!ours) fail("episode was NOT in the queue after enqueue (write did not land)");
const index = afterEnqueue.value.indexOf(ours);
logger.info(`Verified in queue at position ${index + 1}/${afterEnqueue.value.length}`);

// 3. Dequeue (clean up) using the guid the queue itself reports.
const dequeue = await account.dequeueEpisode(ours.episodeGuid as string);
if (dequeue !== "removed") fail(`dequeueEpisode returned "${dequeue}"`);

// 4. Verify it is gone.
const afterDequeue = await account.fetchQueue();
if (afterDequeue.status !== "ok")
  fail(`fetchQueue unavailable: ${afterDequeue.reason}`);
const stillThere = afterDequeue.value.some(
  (item) => item.showTitle === SHOW_TITLE && item.episodeTitle === episode.title,
);
if (stillThere) fail("episode still in the queue after dequeue");

logger.info("SMOKE PASSED: enqueue landed, verified, and cleaned up. Queue untouched.");
process.exit(0);
