import { addBookmark } from "@micthiesen/mitools/karakeep";
import type { Logger } from "@micthiesen/mitools/logging";
import { z } from "zod";
import { enqueueEpisodeJob, type PressPodsJobData } from "./persistence.js";

export const submitEpisodeSchema = z.object({
  // iOS Shortcuts sometimes duplicates the URL with a newline separator
  url: z
    .string()
    .transform((s) => s.split("\n")[0].trim())
    .pipe(z.string().url()),
});

/**
 * Shared submission path for the public endpoint and the web UI: enqueue a
 * durable job, bookmark the article in Karakeep (best-effort), and kick the
 * worker so processing starts immediately instead of at the next sweep.
 */
export function submitEpisodeUrl(
  url: string,
  kickWorker: () => void,
  logger: Logger,
): PressPodsJobData {
  const job = enqueueEpisodeJob(url);
  logger.info(`Episode job enqueued for ${url}`);

  // Fire-and-forget: mitools no-ops without KARAKEEP_URL/KARAKEEP_API_KEY.
  void addBookmark({ url, archived: true, tags: ["PressPods"] }, logger);

  kickWorker();
  return job;
}
