import { randomBytes } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import type { Costs } from "./costs.js";
import type { Chapter, ChunkStat, RetrieverAttempt } from "./types.js";

/**
 * CSPRNG ids: episode ids double as publicly-served audio file names whose
 * only protection is unguessability, so Math.random-based ids are not enough.
 */
export function secureId(): string {
  return randomBytes(16).toString("base64url");
}

export type PressPodsEpisodeData = {
  /** Random id; doubles as the (unguessable) audio file name stem. */
  episodeId: string;
  title: string;
  author?: string;
  authorGender?: "male" | "female" | "unknown";
  publication?: string;
  domain?: string;
  articleUrl: string;
  leadImageUrl?: string;
  excerpt?: string;
  /** The cleaned, narration-ready text that was synthesized. */
  content: string;
  voiceName?: string;
  voiceProvider?: string;
  synthesizedSeconds?: number;
  /** Chapter markers (title + start offset), embedded as ID3 chapters. */
  chapters?: Chapter[];
  /** Per-chunk synthesis stats; absent on episodes created before this field. */
  chunks?: ChunkStat[];
  audioFile: string;
  durationSeconds?: number;
  fileBytes: number;
  retrieverName?: string;
  retrieverSeconds?: number;
  retrieverAttempts?: RetrieverAttempt[];
  costs?: Costs;
  createdAt: number;
  publishedAt?: number;
  /** Task run that produced this episode; links to its captured logs. */
  runId?: string;
};

export const PressPodsEpisodeEntity = new Entity<PressPodsEpisodeData, ["episodeId"]>(
  "press-pods-episode",
  ["episodeId"],
);

export function getAllEpisodes(): PressPodsEpisodeData[] {
  return PressPodsEpisodeEntity.getAll().sort((a, b) => b.createdAt - a.createdAt);
}

export function getEpisode(episodeId: string): PressPodsEpisodeData | undefined {
  return PressPodsEpisodeEntity.get({ episodeId });
}

/**
 * Idempotency probe for crash recovery: an episode for this job's URL created
 * after the job was submitted means the pipeline completed but the process
 * died before the job row was deleted — reprocessing would duplicate it.
 */
export function findEpisodeForJob(
  job: PressPodsJobData,
): PressPodsEpisodeData | undefined {
  return PressPodsEpisodeEntity.getAll().find(
    (episode) => episode.articleUrl === job.url && episode.createdAt >= job.createdAt,
  );
}

// ---------------------------------------------------------------------------
// Job queue (the SQS replacement): submissions become durable rows that the
// PressPods task drains. A crash mid-processing leaves a stale `processing`
// row that the next sweep reclaims; transient TTS failures requeue with
// backoff; permanent failures stay visible in the UI for manual retry.
// ---------------------------------------------------------------------------

export type PressPodsJobStatus = "queued" | "processing" | "failed";

export type PressPodsJobData = {
  jobId: string;
  url: string;
  status: PressPodsJobStatus;
  attempts: number;
  /** Earliest time the job may run (0 = immediately). */
  nextAttemptAt: number;
  lastError?: string;
  createdAt: number;
  updatedAt: number;
  /** Set while processing; used to detect crashed runs. */
  claimedAt?: number;
  lastRunId?: string;
};

export const PressPodsJobEntity = new Entity<PressPodsJobData, ["jobId"]>(
  "press-pods-job",
  ["jobId"],
);

export const MAX_JOB_ATTEMPTS = 6;
const BASE_RETRY_DELAY_MS = 60_000; // 1min, doubling per attempt
/** A `processing` claim older than this is presumed crashed and reclaimed. */
export const STALE_CLAIM_MS = 30 * 60_000;

export function enqueueEpisodeJob(url: string): PressPodsJobData {
  const now = Date.now();
  const job: PressPodsJobData = {
    jobId: secureId(),
    url,
    status: "queued",
    attempts: 0,
    nextAttemptAt: 0,
    createdAt: now,
    updatedAt: now,
  };
  PressPodsJobEntity.upsert(job);
  return job;
}

export function retryDelayMs(attempts: number): number {
  return BASE_RETRY_DELAY_MS * 2 ** Math.max(0, attempts - 1);
}

/** Pure: jobs runnable now — due queued rows plus stale processing claims. */
export function selectDueJobs(
  rows: PressPodsJobData[],
  now = Date.now(),
): PressPodsJobData[] {
  return rows
    .filter(
      (job) =>
        (job.status === "queued" && job.nextAttemptAt <= now) ||
        (job.status === "processing" && (job.claimedAt ?? 0) <= now - STALE_CLAIM_MS),
    )
    .sort((a, b) => a.createdAt - b.createdAt);
}

export function claimJob(jobId: string, runId: string | undefined): void {
  const now = Date.now();
  PressPodsJobEntity.patch(
    { jobId },
    { status: "processing", claimedAt: now, updatedAt: now, lastRunId: runId },
  );
}

export function completeJob(jobId: string): void {
  PressPodsJobEntity.delete({ jobId });
}

/** Requeue with backoff, or mark failed once attempts are exhausted. */
export function recordJobFailure(
  job: PressPodsJobData,
  error: string,
  retryable: boolean,
): PressPodsJobData {
  const now = Date.now();
  // Base the update on the live row, not the caller's pre-claim snapshot —
  // otherwise fields written since selection (e.g. claimJob's lastRunId) are
  // silently wiped. A missing row means a concurrent delete: compute the
  // outcome for the caller's logging but don't resurrect the job.
  const existing = PressPodsJobEntity.get({ jobId: job.jobId });
  const base = existing ?? job;
  const attempts = base.attempts + 1;
  const updated: PressPodsJobData = {
    ...base,
    attempts,
    lastError: error,
    updatedAt: now,
    claimedAt: undefined,
    ...(retryable && attempts < MAX_JOB_ATTEMPTS
      ? { status: "queued" as const, nextAttemptAt: now + retryDelayMs(attempts) }
      : { status: "failed" as const, nextAttemptAt: 0 }),
  };
  if (existing) {
    PressPodsJobEntity.upsert(updated);
  }
  return updated;
}

export function getJob(jobId: string): PressPodsJobData | undefined {
  return PressPodsJobEntity.get({ jobId });
}

/**
 * Manual retry from the UI: reset a failed job to run immediately. Restricted
 * to failed jobs so it can't clobber an in-flight or already-queued attempt.
 */
export function requeueJobNow(jobId: string): PressPodsJobData | undefined {
  const job = PressPodsJobEntity.get({ jobId });
  if (job?.status !== "failed") return undefined;
  const now = Date.now();
  const updated: PressPodsJobData = {
    ...job,
    status: "queued",
    nextAttemptAt: 0,
    claimedAt: undefined,
    updatedAt: now,
  };
  PressPodsJobEntity.upsert(updated);
  return updated;
}

export function getAllJobs(): PressPodsJobData[] {
  return PressPodsJobEntity.getAll().sort((a, b) => b.createdAt - a.createdAt);
}
