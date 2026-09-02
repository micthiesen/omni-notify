import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Clock, Effect } from "effect";
import { getCurrentRunId } from "../task-runs/logCapture.js";
import type { TaskServices } from "../task-runs/registry.js";
import config from "../utils/config.js";
import { isRetryableError, summarizeError } from "./errors.js";
import {
  jobNormalizedUrl,
  MAX_JOB_ATTEMPTS,
  type PressPodsJobData,
  PressPodsPersistence,
  selectDueJobs,
} from "./persistence.js";
import { createEpisodeFromUrl, replaceOlderEpisodes } from "./pipeline.js";
import { errorCause, PressPodsError } from "./effect.js";
import {
  checkpointWorkId,
  clearChunkCheckpoints,
  ensureAudioDir,
  getAudioDir,
} from "./storage.js";

/**
 * Drains the episode job queue. Submissions kick a manual run immediately;
 * the cron sweep is the safety net that picks up backoff retries and jobs
 * orphaned by a crash (stale `processing` claims).
 */
export default class PressPodsTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "PressPods";
  public readonly schedule = "0 */5 * * * *"; // Every 5 minutes
  // Run at boot so a restart immediately drains queued work and recovers jobs
  // orphaned mid-run, rather than waiting up to 5 minutes for the first sweep.
  public readonly runOnStartup = true;

  private logger: Logger;
  private lastRunSummary: string | undefined;
  private reclaimedOnBoot = false;

  public static create(parentLogger: Logger) {
    return Effect.gen(function* () {
      if (!config.PRESSPODS_AUTH_TOKEN) {
        yield* parentLogger.info("PressPods disabled: missing PRESSPODS_AUTH_TOKEN");
        return null;
      }
      const ttsCred: [string, unknown] =
        config.PRESSPODS_TTS_PROVIDER === "elevenlabs"
          ? ["ELEVENLABS_API_KEY", config.ELEVENLABS_API_KEY]
          : ["PRESSPODS_TTS_URL", config.PRESSPODS_TTS_URL];
      const missing = [ttsCred, ...requiredModelCredentials()]
        .filter(([, value]) => !value)
        .map(([name]) => name);
      if (missing.length > 0) {
        yield* parentLogger.info(`PressPods disabled: missing ${missing.join(", ")}`);
        return null;
      }
      return new PressPodsTask(parentLogger);
    });
  }

  private constructor(parentLogger: Logger) {
    this.logger = parentLogger.extend("PressPods");
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }

  public readonly run = Effect.gen({ self: this }, function* () {
    yield* ensureAudioDir().pipe(
      Effect.mapError(
        (cause) =>
          new PressPodsError({
            operation: `prepare audio directory ${getAudioDir()}`,
            cause,
          }),
      ),
    );
    // First run of a fresh process: any `processing` job was orphaned by the
    // restart (single-process deployment), so make its claim immediately
    // reclaimable instead of waiting out the 30-minute stale window. The drain
    // below then either completes it (episode already landed) or counts a
    // crashed attempt and requeues.
    if (!this.reclaimedOnBoot) {
      this.reclaimedOnBoot = true;
      const reclaimed = yield* PressPodsPersistence.reclaimProcessingJobsAtBoot();
      if (reclaimed > 0) {
        yield* this.logger.warn(`Reclaiming ${reclaimed} job(s) orphaned by a restart`);
      }
    }

    let processed = 0;
    let requeued = 0;
    let failed = 0;

    // Drain until nothing is due: jobs submitted while a run is in flight are
    // picked up by the same run instead of waiting for the next sweep.
    for (;;) {
      const now = yield* Clock.currentTimeMillis;
      const due = selectDueJobs(yield* PressPodsPersistence.getAllJobs(), now);
      const job = due[0];
      if (!job) break;

      const outcome = yield* this.processJob(job);
      if (outcome === "processed") processed++;
      else if (outcome === "requeued") requeued++;
      else failed++;
    }

    this.lastRunSummary =
      processed + requeued + failed === 0
        ? "No episode jobs due"
        : `${processed} episode(s) created, ${requeued} requeued, ${failed} failed`;
    if (processed + requeued + failed > 0) {
      yield* this.logger.info(`PressPods pass: ${this.lastRunSummary}`);
    }
  });

  private processJob(job: PressPodsJobData) {
    return Effect.gen({ self: this }, function* () {
      // A stale `processing` claim means a previous run died mid-job. Two cases:
      // the crash happened after the episode was durably written (job cleanup
      // never ran — finish the bookkeeping, never reprocess), or before (count
      // it as an attempt so a job that crashes the process every time still
      // converges to `failed` instead of reclaim-looping forever).
      if (job.status === "processing") {
        const existing = yield* PressPodsPersistence.findEpisodeForJob(job);
        if (existing) {
          yield* this.logger.info(
            `Job for ${job.url} already produced episode ${existing.episodeId}; completing`,
          );
          // Finish the replace + checkpoint cleanup the crashed run never reached,
          // so a crash in the gap between the episode write and cleanup can't
          // leave the article with a permanent duplicate or orphaned checkpoints.
          const normalizedUrl = jobNormalizedUrl(job);
          yield* replaceOlderEpisodes(normalizedUrl, existing.episodeId, this.logger);
          yield* clearChunkCheckpoints(checkpointWorkId(normalizedUrl));
          yield* PressPodsPersistence.completeJob(job.jobId);
          return "processed";
        }
        const updated = yield* PressPodsPersistence.recordJobFailure(
          job,
          "Process crashed or restarted mid-run",
          true,
        );
        if (updated.status === "queued") {
          yield* this.logger.warn(
            `Reclaimed crashed job for ${job.url}; will retry (attempt ${updated.attempts}/${MAX_JOB_ATTEMPTS})`,
          );
          return "requeued";
        }
        yield* this.logger.error(
          `Giving up on ${job.url}: crashed ${updated.attempts} times mid-run`,
        );
        return "failed";
      }

      yield* PressPodsPersistence.claimJob(job.jobId, getCurrentRunId());
      if (job.attempts > 0) {
        yield* this.logger.info(
          `Retrying episode creation (attempt ${job.attempts + 1}/${MAX_JOB_ATTEMPTS})`,
          { url: job.url, lastError: job.lastError },
        );
      } else {
        yield* this.logger.info(`Creating episode for ${job.url}`);
      }

      const result = yield* createEpisodeFromUrl(
        job.url,
        getCurrentRunId(),
        this.logger,
      ).pipe(Effect.result);
      if (result._tag === "Success") {
        yield* PressPodsPersistence.completeJob(job.jobId);
        return "processed";
      } else {
        const cause = errorCause(result.failure);
        const retryable = isRetryableError(cause);
        const summary = summarizeError(cause);
        const updated = yield* PressPodsPersistence.recordJobFailure(
          job,
          summary,
          retryable,
        );
        if (updated.status === "queued") {
          yield* this.logger.warn(
            `Episode creation failed, will retry (attempt ${updated.attempts}/${MAX_JOB_ATTEMPTS})`,
            { url: job.url, error: summary },
          );
          return "requeued";
        }
        yield* this.logger.error(
          `Episode creation failed permanently for ${job.url}`,
          summary,
        );
        return "failed";
      }
    });
  }
}

function requiredModelCredentials(): [string, unknown][] {
  const modelIds = [
    config.PRESSPODS_METADATA_MODEL ?? "openai:gpt-5.6-luna",
    config.PRESSPODS_CLEANING_MODEL ?? "openai:gpt-5.6-terra",
  ];
  const providers = new Set(modelIds.map((id) => id.split(":", 1)[0]));
  const credentials: [string, unknown][] = [];
  if (providers.has("google")) {
    credentials.push([
      "GOOGLE_GENERATIVE_AI_API_KEY",
      config.GOOGLE_GENERATIVE_AI_API_KEY,
    ]);
  }
  if (providers.has("openai")) {
    credentials.push(["OPENAI_API_KEY", config.OPENAI_API_KEY]);
  }
  if (providers.has("anthropic")) {
    credentials.push(["ANTHROPIC_API_KEY", config.ANTHROPIC_API_KEY]);
  }
  return credentials;
}
