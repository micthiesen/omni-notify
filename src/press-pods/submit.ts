import { addBookmark } from "@micthiesen/mitools/karakeep";
import type { Logger } from "@micthiesen/mitools/logging";
import { z } from "zod";
import {
  enqueueEpisodeJob,
  findActiveJobByNormalizedUrl,
  findFailedJobByNormalizedUrl,
  type PressPodsJobData,
  requeueJobNow,
} from "./persistence.js";
import { normalizeUrl } from "./url.js";

export const submitEpisodeSchema = z.object({
  // iOS Shortcuts sometimes duplicates the URL with a newline separator
  url: z
    .string()
    .transform((s) => s.split("\n")[0].trim())
    .pipe(z.string().url()),
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
export function submitEpisodeUrl(
  url: string,
  kickWorker: () => void,
  logger: Logger,
): PressPodsJobData {
  const normalizedUrl = normalizeUrl(url);

  const active = findActiveJobByNormalizedUrl(normalizedUrl);
  if (active) {
    logger.info(`Episode job already ${active.status} for ${url}; joining it`);
    kickWorker();
    return active;
  }

  const failed = findFailedJobByNormalizedUrl(normalizedUrl);
  if (failed) {
    const requeued = requeueJobNow(failed.jobId);
    if (requeued) {
      logger.info(`Retrying previously-failed episode job for ${url}`);
      kickWorker();
      return requeued;
    }
  }

  const job = enqueueEpisodeJob(url);
  logger.info(`Episode job enqueued for ${url}`);

  // Fire-and-forget: mitools no-ops without KARAKEEP_URL/KARAKEEP_API_KEY.
  void addBookmark({ url, archived: true, tags: ["PressPods"] }, logger);

  kickWorker();
  return job;
}
