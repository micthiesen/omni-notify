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
import { Data, Effect } from "effect";
import { runPromise } from "../effect/interop.js";
import {
  PodcastQueuePosition,
  resolvePodcastAccountEffect,
} from "../podcast-recs/account.js";
import { fetchFeedEpisodesEffect } from "../podcast-recs/rss.js";
import config from "../utils/config.js";

Injector.configure({ config });
const logger = new Logger("CastroSmoke");

// Radiolab: a show the owner does not subscribe to (Simplecast feed whose RSS
// guids differ from Castro's, so this also covers media-URL matching).
const SHOW_TITLE = "Radiolab";
const FEED_URL = "https://feeds.simplecast.com/EmVW7VGp";

class SmokeError extends Data.TaggedError("SmokeError")<{
  message: string;
  cause?: unknown;
}> {}
const fail = (message: string, cause?: unknown) =>
  Effect.fail(new SmokeError({ message: `SMOKE FAILED: ${message}`, cause }));
const program = Effect.gen(function* () {
  const account = yield* resolvePodcastAccountEffect(logger);
  if (!account)
    return yield* fail(
      "no Castro account configured (CASTRO_ACCESS_ID/CASTRO_SECRET_KEY)",
    );

  const episode = (yield* fetchFeedEpisodesEffect(FEED_URL, { maxEpisodes: 1 }).pipe(
    Effect.mapError(
      (cause) =>
        new SmokeError({
          message: "SMOKE FAILED: could not read the test feed",
          cause,
        }),
    ),
  ))[0];
  if (!episode) return yield* fail("could not read the test feed");
  yield* Effect.sync(() =>
    logger.info(`Test episode: ${SHOW_TITLE} — ${episode.title}`),
  );

  const enqueue = yield* account.enqueueEpisode({
    feedUrl: FEED_URL,
    episodeGuid: episode.guid,
    mediaUrl: episode.enclosureUrl,
    showTitle: SHOW_TITLE,
    episodeTitle: episode.title,
    position: PodcastQueuePosition.Next,
  });
  if (enqueue !== "added" && enqueue !== "already_exists") {
    return yield* fail(`enqueueEpisode returned "${enqueue}"`);
  }
  yield* Effect.sync(() => logger.info(`enqueueEpisode → ${enqueue}`));

  // 2. Verify it actually landed in the server queue (not just a 200).
  const afterEnqueue = yield* account.fetchQueue();
  if (afterEnqueue.status !== "ok")
    return yield* fail(`fetchQueue unavailable: ${afterEnqueue.reason}`);
  const ours = afterEnqueue.value.find(
    (item) => item.showTitle === SHOW_TITLE && item.episodeTitle === episode.title,
  );
  if (!ours)
    return yield* fail(
      "episode was NOT in the queue after enqueue (write did not land)",
    );
  const index = afterEnqueue.value.indexOf(ours);
  yield* Effect.sync(() =>
    logger.info(
      `Verified in queue at position ${index + 1}/${afterEnqueue.value.length}`,
    ),
  );

  // 3. Dequeue (clean up) using the guid the queue itself reports.
  const dequeue = yield* account.dequeueEpisode(ours.episodeGuid as string);
  if (dequeue !== "removed") return yield* fail(`dequeueEpisode returned "${dequeue}"`);

  // 4. Verify it is gone.
  const afterDequeue = yield* account.fetchQueue();
  if (afterDequeue.status !== "ok")
    return yield* fail(`fetchQueue unavailable: ${afterDequeue.reason}`);
  const stillThere = afterDequeue.value.some(
    (item) => item.showTitle === SHOW_TITLE && item.episodeTitle === episode.title,
  );
  if (stillThere) return yield* fail("episode still in the queue after dequeue");

  yield* Effect.sync(() =>
    logger.info(
      "SMOKE PASSED: enqueue landed, verified, and cleaned up. Queue untouched.",
    ),
  );
});

await runPromise(
  program.pipe(
    Effect.tapError((error) =>
      Effect.sync(() => logger.error(error.message, error.cause)),
    ),
  ),
);
