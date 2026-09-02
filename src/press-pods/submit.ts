import { addBookmark } from "@micthiesen/mitools/karakeep";
import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { z } from "zod";
import { PressPodsPersistence } from "./persistence.js";
import { assertPublicHttpUrl, assertPublicHttpUrlSyntax } from "./publicHttp.js";
import { normalizeUrl } from "./url.js";

export const submitEpisodeSchema = z.object({
  // iOS Shortcuts sometimes duplicates the URL with a newline separator
  url: z
    .string()
    .transform((s) => s.split("\n")[0].trim())
    .pipe(z.string().url())
    .superRefine((url, context) => {
      try {
        assertPublicHttpUrlSyntax(url);
      } catch (error) {
        context.addIssue({
          code: "custom",
          message: error instanceof Error ? error.message : "URL must be public",
        });
      }
    }),
});

/**
 * Shared submission path for the public endpoint and the web UI. Resubmitting a
 * URL is treated as a retry rather than a new entry: if a job for the same
 * canonical URL (see url.ts) is already queued or processing we join it, and a
 * failed one is requeued to run now — so re-submitting never stacks duplicate
 * jobs. A URL that already produced an episode still enqueues a fresh job; the
 * pipeline replaces the older episode on completion. Otherwise we enqueue,
 * bookmark the article in Karakeep (best-effort), and kick the worker so
 * processing starts immediately instead of at the next sweep.
 */
export function submitEpisodeUrlEffect<E, R>(
  url: string,
  kickWorker: () => Effect.Effect<unknown, E, R>,
  logger: Logger,
) {
  return Effect.gen(function* () {
    const publicUrl = yield* assertPublicHttpUrl(url);
    const validatedUrl = publicUrl.toString();
    const normalizedUrl = normalizeUrl(validatedUrl);

    const active =
      yield* PressPodsPersistence.findActiveJobByNormalizedUrl(normalizedUrl);
    if (active) {
      yield* logger.info(
        `Episode job already ${active.status} for ${validatedUrl}; joining it`,
      );
      yield* kickWorker();
      return active;
    }

    const failed =
      yield* PressPodsPersistence.findFailedJobByNormalizedUrl(normalizedUrl);
    if (failed) {
      const requeued = yield* PressPodsPersistence.requeueJobNow(failed.jobId);
      if (requeued) {
        yield* logger.info(
          `Retrying previously-failed episode job for ${validatedUrl}`,
        );
        yield* kickWorker();
        return requeued;
      }
    }

    const job = yield* PressPodsPersistence.enqueueEpisodeJob(validatedUrl);
    yield* logger.info(`Episode job enqueued for ${validatedUrl}`);

    // Bookmarking remains best-effort, but is kept inside the structured workflow
    // so interruption and failures cannot leave an unobserved Promise behind.
    yield* addBookmark({ url: validatedUrl, archived: true, tags: ["PressPods"] }).pipe(
      Effect.catch(() => Effect.void),
    );

    yield* kickWorker();
    return job;
  });
}
