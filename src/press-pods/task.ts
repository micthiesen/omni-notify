import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { getCurrentRunId } from "../task-runs/logCapture.js";
import config from "../utils/config.js";
import { isRetryableError, summarizeError } from "./errors.js";
import {
  claimJob,
  completeJob,
  findEpisodeForJob,
  getAllJobs,
  MAX_JOB_ATTEMPTS,
  type PressPodsJobData,
  recordJobFailure,
  selectDueJobs,
} from "./persistence.js";
import { createEpisodeFromUrl } from "./pipeline.js";
import { ensureAudioDir, getAudioDir } from "./storage.js";

/**
 * Drains the episode job queue. Submissions kick a manual run immediately;
 * the cron sweep is the safety net that picks up backoff retries and jobs
 * orphaned by a crash (stale `processing` claims).
 */
export default class PressPodsTask extends ScheduledTask {
  public readonly name = "PressPods";
  public readonly schedule = "0 */5 * * * *"; // Every 5 minutes

  private logger: Logger;
  private lastRunSummary: string | undefined;

  public static create(parentLogger: Logger): PressPodsTask | null {
    if (!config.PRESSPODS_AUTH_TOKEN) {
      parentLogger.info("PressPods disabled: missing PRESSPODS_AUTH_TOKEN");
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
      parentLogger.info(`PressPods disabled: missing ${missing.join(", ")}`);
      return null;
    }
    // A bad audio dir must disable the feature, not crash-loop the whole app
    // at boot; the warn reaches Pushover so the misconfiguration is loud.
    try {
      ensureAudioDir();
    } catch (error) {
      parentLogger.warn(
        `PressPods disabled: cannot create audio dir "${getAudioDir()}": ${(error as Error).message}`,
      );
      return null;
    }
    return new PressPodsTask(parentLogger);
  }

  private constructor(parentLogger: Logger) {
    super();
    this.logger = parentLogger.extend("PressPods");
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }

  public async run(): Promise<void> {
    let processed = 0;
    let requeued = 0;
    let failed = 0;

    // Drain until nothing is due: jobs submitted while a run is in flight are
    // picked up by the same run instead of waiting for the next sweep.
    for (;;) {
      const due = selectDueJobs(getAllJobs());
      const job = due[0];
      if (!job) break;

      const outcome = await this.processJob(job);
      if (outcome === "processed") processed++;
      else if (outcome === "requeued") requeued++;
      else failed++;
    }

    this.lastRunSummary =
      processed + requeued + failed === 0
        ? "No episode jobs due"
        : `${processed} episode(s) created, ${requeued} requeued, ${failed} failed`;
    if (processed + requeued + failed > 0) {
      this.logger.info(`PressPods pass: ${this.lastRunSummary}`);
    }
  }

  private async processJob(
    job: PressPodsJobData,
  ): Promise<"processed" | "requeued" | "failed"> {
    // A stale `processing` claim means a previous run died mid-job. Two cases:
    // the crash happened after the episode was durably written (job cleanup
    // never ran — finish the bookkeeping, never reprocess), or before (count
    // it as an attempt so a job that crashes the process every time still
    // converges to `failed` instead of reclaim-looping forever).
    if (job.status === "processing") {
      const existing = findEpisodeForJob(job);
      if (existing) {
        this.logger.info(
          `Job for ${job.url} already produced episode ${existing.episodeId}; completing`,
        );
        completeJob(job.jobId);
        return "processed";
      }
      const updated = recordJobFailure(
        job,
        "Process crashed or restarted mid-run",
        true,
      );
      if (updated.status === "queued") {
        this.logger.warn(
          `Reclaimed crashed job for ${job.url}; will retry (attempt ${updated.attempts}/${MAX_JOB_ATTEMPTS})`,
        );
        return "requeued";
      }
      this.logger.error(
        `Giving up on ${job.url}: crashed ${updated.attempts} times mid-run`,
      );
      return "failed";
    }

    claimJob(job.jobId, getCurrentRunId());
    if (job.attempts > 0) {
      this.logger.info(
        `Retrying episode creation (attempt ${job.attempts + 1}/${MAX_JOB_ATTEMPTS})`,
        { url: job.url, lastError: job.lastError },
      );
    } else {
      this.logger.info(`Creating episode for ${job.url}`);
    }

    try {
      await createEpisodeFromUrl(job.url, getCurrentRunId(), this.logger);
      completeJob(job.jobId);
      return "processed";
    } catch (error) {
      const retryable = isRetryableError(error);
      const summary = summarizeError(error);
      const updated = recordJobFailure(job, summary, retryable);
      if (updated.status === "queued") {
        this.logger.warn(
          `Episode creation failed, will retry (attempt ${updated.attempts}/${MAX_JOB_ATTEMPTS})`,
          { url: job.url, error: summary },
        );
        return "requeued";
      }
      this.logger.error(`Episode creation failed permanently for ${job.url}`, summary);
      return "failed";
    }
  }
}

function requiredModelCredentials(): [string, unknown][] {
  const modelIds = [
    config.PRESSPODS_METADATA_MODEL ?? "google:gemini-3.5-flash",
    config.PRESSPODS_CLEANING_MODEL ?? "google:gemini-3.5-flash",
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
