import { randomBytes } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import { Clock, Effect, Option, Schema } from "effect";
import type { Costs } from "./costs.js";
import { PressPodsError } from "./effect.js";
import type { Chapter, ChunkStat, RetrieverAttempt } from "./types.js";
import { normalizeUrl } from "./url.js";

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
  /** Canonical identity for dedup/replace (see url.ts); absent on old rows. */
  normalizedUrl?: string;
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

export const getAllEpisodes = Effect.fn("PressPods.getAllEpisodes")(function* () {
  return (yield* PressPodsEpisodeEntity.getAll()).sort(
    (a, b) => b.createdAt - a.createdAt,
  );
});

export const getEpisode = Effect.fn("PressPods.getEpisode")(function* (
  episodeId: string,
) {
  return Option.getOrUndefined(yield* PressPodsEpisodeEntity.get({ episodeId }));
});

/**
 * Idempotency probe for crash recovery: an episode for this job's URL created
 * after the job was submitted means the pipeline completed but the process
 * died before the job row was deleted — reprocessing would duplicate it.
 */
export const findEpisodeForJob = Effect.fn("PressPods.findEpisodeForJob")(function* (
  job: PressPodsJobData,
) {
  const normalized = jobNormalizedUrl(job);
  return (yield* PressPodsEpisodeEntity.getAll()).find(
    (episode) =>
      episodeNormalizedUrl(episode) === normalized &&
      episode.createdAt >= job.createdAt,
  );
});

// ---------------------------------------------------------------------------
// Job queue (the SQS replacement): submissions become durable rows that the
// PressPods task drains. A crash mid-processing leaves a stale `processing`
// row that the next sweep reclaims; transient TTS failures requeue with
// backoff; permanent failures stay visible in the UI for manual retry.
// ---------------------------------------------------------------------------

export type PressPodsJobStatus = "queued" | "processing" | "failed";

export type PressPodsJobData = {
  jobId: string;
  /** Original submitted URL — what the retrievers fetch (query string intact). */
  url: string;
  /** Canonical identity for dedup/resubmit-as-retry (see url.ts). */
  normalizedUrl?: string;
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

export const enqueueEpisodeJob = Effect.fn("PressPods.enqueueJob")(function* (
  url: string,
  now?: number,
) {
  const timestamp = now ?? (yield* Clock.currentTimeMillis);
  const job: PressPodsJobData = {
    jobId: secureId(),
    url,
    normalizedUrl: normalizeUrl(url),
    status: "queued",
    attempts: 0,
    nextAttemptAt: 0,
    createdAt: timestamp,
    updatedAt: timestamp,
  };
  yield* PressPodsJobEntity.upsert(job);
  return job;
});

/** Canonical identity for a job/episode, computed on read so pre-existing rows
 * (written before `normalizedUrl` was stored) still dedup correctly. */
export function jobNormalizedUrl(job: PressPodsJobData): string {
  return job.normalizedUrl ?? normalizeUrl(job.url);
}
export function episodeNormalizedUrl(episode: PressPodsEpisodeData): string {
  return episode.normalizedUrl ?? normalizeUrl(episode.articleUrl);
}

/** An in-flight job (queued or processing) for this URL — a resubmit joins it
 * instead of enqueueing a duplicate. Newest first. */
export const findActiveJobByNormalizedUrl = Effect.fn("PressPods.findActiveJob")(
  function* (normalizedUrl: string) {
    return (yield* getAllJobs()).find(
      (j) =>
        (j.status === "queued" || j.status === "processing") &&
        jobNormalizedUrl(j) === normalizedUrl,
    );
  },
);

/** A failed job for this URL — a resubmit requeues it rather than stacking. */
export const findFailedJobByNormalizedUrl = Effect.fn("PressPods.findFailedJob")(
  function* (normalizedUrl: string) {
    return (yield* getAllJobs()).find(
      (j) => j.status === "failed" && jobNormalizedUrl(j) === normalizedUrl,
    );
  },
);

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

export const claimJob = Effect.fn("PressPods.claimJob")(function* (
  jobId: string,
  runId: string | undefined,
  now?: number,
) {
  const timestamp = now ?? (yield* Clock.currentTimeMillis);
  yield* PressPodsJobEntity.patch(
    { jobId },
    {
      status: "processing",
      claimedAt: timestamp,
      updatedAt: timestamp,
      lastRunId: runId,
    },
  );
});

export const completeJob = Effect.fn("PressPods.completeJob")(function* (
  jobId: string,
) {
  yield* PressPodsJobEntity.delete({ jobId });
});

/** Requeue with backoff, or mark failed once attempts are exhausted. */
export const recordJobFailure = Effect.fn("PressPods.recordJobFailure")(function* (
  job: PressPodsJobData,
  error: string,
  retryable: boolean,
  now?: number,
) {
  const timestamp = now ?? (yield* Clock.currentTimeMillis);
  // Base the update on the live row, not the caller's pre-claim snapshot —
  // otherwise fields written since selection (e.g. claimJob's lastRunId) are
  // silently wiped. A missing row means a concurrent delete: compute the
  // outcome for the caller's logging but don't resurrect the job.
  const existing = Option.getOrUndefined(
    yield* PressPodsJobEntity.get({ jobId: job.jobId }),
  );
  const base = existing ?? job;
  const attempts = base.attempts + 1;
  const updated: PressPodsJobData = {
    ...base,
    attempts,
    lastError: error,
    updatedAt: timestamp,
    claimedAt: undefined,
    ...(retryable && attempts < MAX_JOB_ATTEMPTS
      ? { status: "queued" as const, nextAttemptAt: timestamp + retryDelayMs(attempts) }
      : { status: "failed" as const, nextAttemptAt: 0 }),
  };
  if (existing) {
    yield* PressPodsJobEntity.upsert(updated);
  }
  return updated;
});

export const getJob = Effect.fn("PressPods.getJob")(function* (jobId: string) {
  return Option.getOrUndefined(yield* PressPodsJobEntity.get({ jobId }));
});

/**
 * Manual retry from the UI (or a resubmit joining a failed job): reset a failed
 * job to run immediately with a fresh attempt budget. Restricted to failed jobs
 * so it can't clobber an in-flight or already-queued attempt. `attempts` is
 * reset to 0 — a deliberate user retry earns a full retry cycle, not the single
 * shot a still-`MAX_JOB_ATTEMPTS` counter would leave (which would re-fail on
 * the next transient blip).
 */
export const requeueJobNow = Effect.fn("PressPods.requeueJobNow")(function* (
  jobId: string,
  now?: number,
) {
  const timestamp = now ?? (yield* Clock.currentTimeMillis);
  const job = Option.getOrUndefined(yield* PressPodsJobEntity.get({ jobId }));
  if (job?.status !== "failed") return undefined;
  const updated: PressPodsJobData = {
    ...job,
    status: "queued",
    attempts: 0,
    nextAttemptAt: 0,
    claimedAt: undefined,
    lastError: undefined,
    updatedAt: timestamp,
  };
  yield* PressPodsJobEntity.upsert(updated);
  return updated;
});

export const getAllJobs = Effect.fn("PressPods.getAllJobs")(function* () {
  return (yield* PressPodsJobEntity.getAll()).sort((a, b) => b.createdAt - a.createdAt);
});

/**
 * Boot-time crash recovery. The deployment is single-process, so any job left
 * in `processing` when a fresh process starts was orphaned by an abrupt
 * restart — its worker didn't survive. Mark those claims immediately stale so
 * the very next drain reclaims them (the normal `processing` branch then either
 * completes the job if its episode already landed, or counts a crashed attempt
 * and requeues) instead of waiting out the 30-minute stale window. Returns the
 * number of orphaned jobs found.
 */
export const reclaimProcessingJobsAtBoot = Effect.fn("PressPods.reclaimAtBoot")(
  function* () {
    const orphaned = (yield* PressPodsJobEntity.getAll()).filter(
      (j) => j.status === "processing",
    );
    for (const job of orphaned) {
      yield* PressPodsJobEntity.patch({ jobId: job.jobId }, { claimedAt: 0 });
    }
    return orphaned.length;
  },
);

/** Remove an episode row. Returns the deleted row so the caller can clean up
 * its audio file (kept separate — persistence stays free of filesystem I/O). */
export const deleteEpisode = Effect.fn("PressPods.deleteEpisode")(function* (
  episodeId: string,
) {
  const episode = Option.getOrUndefined(
    yield* PressPodsEpisodeEntity.get({ episodeId }),
  );
  if (!episode) return undefined;
  yield* PressPodsEpisodeEntity.delete({ episodeId });
  return episode;
});

/**
 * Replace semantics for resubmit-as-retry: after a fresh episode is written for
 * a URL, drop any older episodes sharing its canonical identity so the newest
 * take wins instead of piling up duplicates. Returns the removed rows so the
 * caller can delete their audio files.
 */
export const deleteEpisodesByNormalizedUrlExcept = Effect.fn(
  "PressPods.deleteEpisodesByUrl",
)(function* (normalizedUrl: string, keepEpisodeId: string) {
  const stale = (yield* PressPodsEpisodeEntity.getAll()).filter(
    (e) => e.episodeId !== keepEpisodeId && episodeNormalizedUrl(e) === normalizedUrl,
  );
  for (const episode of stale) {
    yield* PressPodsEpisodeEntity.delete({ episodeId: episode.episodeId });
  }
  return stale;
});

const PressPodsEpisodeSchema = Schema.Struct({
  episodeId: Schema.String,
  title: Schema.String,
  author: Schema.optional(Schema.String),
  authorGender: Schema.optional(Schema.Literals(["male", "female", "unknown"])),
  publication: Schema.optional(Schema.String),
  domain: Schema.optional(Schema.String),
  articleUrl: Schema.String,
  normalizedUrl: Schema.optional(Schema.String),
  leadImageUrl: Schema.optional(Schema.String),
  excerpt: Schema.optional(Schema.String),
  content: Schema.String,
  voiceName: Schema.optional(Schema.String),
  voiceProvider: Schema.optional(Schema.String),
  synthesizedSeconds: Schema.optional(Schema.Number),
  chapters: Schema.optional(
    Schema.mutable(
      Schema.Array(
        Schema.Struct({
          startTimeSeconds: Schema.Number,
          title: Schema.String,
        }),
      ),
    ),
  ),
  chunks: Schema.optional(
    Schema.mutable(
      Schema.Array(
        Schema.Struct({
          index: Schema.Number,
          sectionIndex: Schema.Number,
          sectionTitle: Schema.optional(Schema.String),
          text: Schema.String,
          charCount: Schema.Number,
          durationSeconds: Schema.Number,
          startTimeSeconds: Schema.Number,
          secPerChar: Schema.Number,
          attempts: Schema.Number,
          coverage: Schema.optional(Schema.Number),
          wordRatio: Schema.optional(Schema.Number),
          expectedWords: Schema.optional(Schema.Number),
          resplit: Schema.optional(Schema.Boolean),
          resplitDepth: Schema.optional(Schema.Number),
        }),
      ),
    ),
  ),
  audioFile: Schema.String,
  durationSeconds: Schema.optional(Schema.Number),
  fileBytes: Schema.Number,
  retrieverName: Schema.optional(Schema.String),
  retrieverSeconds: Schema.optional(Schema.Number),
  retrieverAttempts: Schema.optional(
    Schema.mutable(
      Schema.Array(
        Schema.Union([
          Schema.Struct({
            name: Schema.String,
            success: Schema.Literal(true),
            contentRating: Schema.Number,
            textChars: Schema.Number,
          }),
          Schema.Struct({
            name: Schema.String,
            success: Schema.Literal(false),
            error: Schema.String,
          }),
        ]),
      ),
    ),
  ),
  costs: Schema.optional(
    Schema.Struct({
      llmCents: Schema.Number,
      ttsCents: Schema.Number,
      detailCents: Schema.Record(Schema.String, Schema.Number),
      detailTokens: Schema.Record(
        Schema.String,
        Schema.Struct({ input: Schema.Number, output: Schema.Number }),
      ),
      detailChars: Schema.Record(Schema.String, Schema.Number),
    }),
  ),
  createdAt: Schema.Number,
  publishedAt: Schema.optional(Schema.Number),
  runId: Schema.optional(Schema.String),
});

const PressPodsJobSchema = Schema.Struct({
  jobId: Schema.String,
  url: Schema.String,
  normalizedUrl: Schema.optional(Schema.String),
  status: Schema.Literals(["queued", "processing", "failed"]),
  attempts: Schema.Number,
  nextAttemptAt: Schema.Number,
  lastError: Schema.optional(Schema.String),
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
  claimedAt: Schema.optional(Schema.Number),
  lastRunId: Schema.optional(Schema.String),
});

const decodeEpisode = (
  value: unknown,
): Effect.Effect<PressPodsEpisodeData, PressPodsError> =>
  Schema.decodeUnknownEffect(PressPodsEpisodeSchema)(value).pipe(
    Effect.mapError(
      (cause) => new PressPodsError({ operation: "decode PressPods episode", cause }),
    ),
  );

const decodeJob = (value: unknown): Effect.Effect<PressPodsJobData, PressPodsError> =>
  Schema.decodeUnknownEffect(PressPodsJobSchema)(value).pipe(
    Effect.map((decoded) => decoded as PressPodsJobData),
    Effect.mapError(
      (cause) => new PressPodsError({ operation: "decode PressPods job", cause }),
    ),
  );

/** Canonical application API. */
export const PressPodsPersistence = {
  getAllEpisodes: () =>
    getAllEpisodes().pipe(
      Effect.flatMap((rows) => Effect.forEach(rows, decodeEpisode)),
    ),
  getEpisode: (episodeId: string) =>
    getEpisode(episodeId).pipe(
      Effect.flatMap((row) => (row ? decodeEpisode(row) : Effect.succeed(undefined))),
    ),
  findEpisodeForJob: (job: PressPodsJobData) =>
    findEpisodeForJob(job).pipe(
      Effect.flatMap((row) => (row ? decodeEpisode(row) : Effect.succeed(undefined))),
    ),
  upsertEpisode: (episode: PressPodsEpisodeData) =>
    PressPodsEpisodeEntity.upsert(episode).pipe(Effect.asVoid),
  enqueueEpisodeJob,
  findActiveJobByNormalizedUrl,
  findFailedJobByNormalizedUrl,
  claimJob,
  completeJob,
  recordJobFailure: (job: PressPodsJobData, error: string, retryable: boolean) =>
    recordJobFailure(job, error, retryable),
  getJob: (jobId: string) =>
    getJob(jobId).pipe(
      Effect.flatMap((row) => (row ? decodeJob(row) : Effect.succeed(undefined))),
    ),
  requeueJobNow,
  getAllJobs: () =>
    getAllJobs().pipe(Effect.flatMap((rows) => Effect.forEach(rows, decodeJob))),
  reclaimProcessingJobsAtBoot,
  deleteEpisode,
  deleteJob: (jobId: string) =>
    PressPodsJobEntity.delete({ jobId }).pipe(Effect.asVoid),
  deleteEpisodesByNormalizedUrlExcept: (normalizedUrl: string, keepEpisodeId: string) =>
    deleteEpisodesByNormalizedUrlExcept(normalizedUrl, keepEpisodeId),
} as const;
