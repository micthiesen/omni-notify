import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it } from "vitest";
import {
  deleteEpisodesByNormalizedUrlExcept,
  findActiveJobByNormalizedUrl,
  findEpisodeForJob,
  findFailedJobByNormalizedUrl,
  MAX_JOB_ATTEMPTS,
  type PressPodsEpisodeData,
  PressPodsEpisodeEntity,
  type PressPodsJobData,
  PressPodsJobEntity,
  reclaimProcessingJobsAtBoot,
  recordJobFailure,
  requeueJobNow,
  retryDelayMs,
  STALE_CLAIM_MS,
  secureId,
  selectDueJobs,
} from "./persistence.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "presspods-persistence.spec.db",
  },
});

const NOW = 1_700_000_000_000;

function job(overrides: Partial<PressPodsJobData>): PressPodsJobData {
  return {
    jobId: "j1",
    url: "https://example.com/a",
    status: "queued",
    attempts: 0,
    nextAttemptAt: 0,
    createdAt: NOW - 60_000,
    updatedAt: NOW - 60_000,
    ...overrides,
  };
}

describe("selectDueJobs", () => {
  it("selects queued jobs that are due", () => {
    const due = selectDueJobs([job({})], NOW);
    expect(due).toHaveLength(1);
  });

  it("excludes queued jobs with a future nextAttemptAt", () => {
    const due = selectDueJobs([job({ nextAttemptAt: NOW + 60_000 })], NOW);
    expect(due).toHaveLength(0);
  });

  it("reclaims stale processing claims", () => {
    const due = selectDueJobs(
      [job({ status: "processing", claimedAt: NOW - STALE_CLAIM_MS - 1 })],
      NOW,
    );
    expect(due).toHaveLength(1);
  });

  it("leaves fresh processing claims alone", () => {
    const due = selectDueJobs(
      [job({ status: "processing", claimedAt: NOW - 60_000 })],
      NOW,
    );
    expect(due).toHaveLength(0);
  });

  it("excludes failed jobs", () => {
    const due = selectDueJobs([job({ status: "failed" })], NOW);
    expect(due).toHaveLength(0);
  });

  it("orders by submission time", () => {
    const due = selectDueJobs(
      [
        job({ jobId: "newer", createdAt: NOW - 1000 }),
        job({ jobId: "older", createdAt: NOW - 2000 }),
      ],
      NOW,
    );
    expect(due.map((j) => j.jobId)).toEqual(["older", "newer"]);
  });
});

describe("retryDelayMs", () => {
  it("doubles per attempt", () => {
    expect(retryDelayMs(1)).toBe(60_000);
    expect(retryDelayMs(2)).toBe(120_000);
    expect(retryDelayMs(3)).toBe(240_000);
  });
});

describe("recordJobFailure", () => {
  // recordJobFailure reads the live row, so stale rows from previous test
  // runs (the spec DB persists on disk) must be cleared.
  beforeEach(() => {
    for (const jobId of ["r1", "r2", "r3", "gone"]) {
      PressPodsJobEntity.delete({ jobId });
    }
  });

  it("requeues a retryable failure with backoff", () => {
    const updated = recordJobFailure(job({ jobId: "r1" }), "boom", true);
    expect(updated.status).toBe("queued");
    expect(updated.attempts).toBe(1);
    expect(updated.nextAttemptAt).toBeGreaterThan(Date.now() - 1000);
    expect(updated.lastError).toBe("boom");
  });

  it("fails permanently on a non-retryable error", () => {
    const updated = recordJobFailure(job({ jobId: "r2" }), "bad article", false);
    expect(updated.status).toBe("failed");
  });

  it("fails permanently once attempts are exhausted", () => {
    const updated = recordJobFailure(
      job({ jobId: "r3", attempts: MAX_JOB_ATTEMPTS - 1 }),
      "still broken",
      true,
    );
    expect(updated.status).toBe("failed");
    expect(updated.attempts).toBe(MAX_JOB_ATTEMPTS);
  });

  it("does not resurrect a concurrently-deleted job", () => {
    const deleted = job({ jobId: "gone" });
    recordJobFailure(deleted, "boom", true);
    expect(PressPodsJobEntity.get({ jobId: "gone" })).toBeUndefined();
  });
});

describe("requeueJobNow", () => {
  it("requeues a failed job immediately", () => {
    PressPodsJobEntity.upsert(job({ jobId: "f1", status: "failed" }));
    const updated = requeueJobNow("f1");
    expect(updated?.status).toBe("queued");
    expect(updated?.nextAttemptAt).toBe(0);
    PressPodsJobEntity.delete({ jobId: "f1" });
  });

  it("refuses non-failed jobs", () => {
    PressPodsJobEntity.upsert(job({ jobId: "q1", status: "processing" }));
    expect(requeueJobNow("q1")).toBeUndefined();
    PressPodsJobEntity.delete({ jobId: "q1" });
  });

  it("resets the attempt budget so an exhausted job gets a fresh retry cycle", () => {
    PressPodsJobEntity.upsert(
      job({
        jobId: "f2",
        status: "failed",
        attempts: MAX_JOB_ATTEMPTS,
        lastError: "boom",
      }),
    );
    const updated = requeueJobNow("f2");
    expect(updated?.attempts).toBe(0);
    expect(updated?.lastError).toBeUndefined();
    PressPodsJobEntity.delete({ jobId: "f2" });
  });
});

describe("findEpisodeForJob", () => {
  const episode = (overrides: Partial<PressPodsEpisodeData>): PressPodsEpisodeData => ({
    episodeId: secureId(),
    title: "t",
    articleUrl: "https://example.com/a",
    content: "c",
    audioFile: "a.mp3",
    fileBytes: 1,
    createdAt: NOW,
    ...overrides,
  });

  it("finds an episode created after the job was submitted", () => {
    const row = episode({ createdAt: NOW });
    PressPodsEpisodeEntity.upsert(row);
    expect(findEpisodeForJob(job({ createdAt: NOW - 1000 }))?.episodeId).toBe(
      row.episodeId,
    );
    PressPodsEpisodeEntity.delete({ episodeId: row.episodeId });
  });

  it("ignores older episodes for the same URL (resubmissions)", () => {
    const row = episode({ createdAt: NOW - 60_000 });
    PressPodsEpisodeEntity.upsert(row);
    expect(findEpisodeForJob(job({ createdAt: NOW - 1000 }))).toBeUndefined();
    PressPodsEpisodeEntity.delete({ episodeId: row.episodeId });
  });

  it("matches on canonical identity despite tracking-param differences", () => {
    const row = episode({
      createdAt: NOW,
      articleUrl: "https://example.com/story?utm_source=rss",
    });
    PressPodsEpisodeEntity.upsert(row);
    const found = findEpisodeForJob(
      job({ createdAt: NOW - 1000, url: "https://example.com/story?ref=twitter" }),
    );
    expect(found?.episodeId).toBe(row.episodeId);
    PressPodsEpisodeEntity.delete({ episodeId: row.episodeId });
  });
});

describe("URL-based dedup lookups", () => {
  const URL_A = "https://dedup.example/piece?utm_source=x";
  const NORM = "https://dedup.example/piece";

  beforeEach(() => {
    for (const jobId of ["active", "failed"]) PressPodsJobEntity.delete({ jobId });
  });

  it("finds a queued or processing job by canonical URL", () => {
    PressPodsJobEntity.upsert(
      job({ jobId: "active", url: URL_A, status: "processing" }),
    );
    expect(findActiveJobByNormalizedUrl(NORM)?.jobId).toBe("active");
    expect(findFailedJobByNormalizedUrl(NORM)).toBeUndefined();
    PressPodsJobEntity.delete({ jobId: "active" });
  });

  it("finds a failed job by canonical URL", () => {
    PressPodsJobEntity.upsert(job({ jobId: "failed", url: URL_A, status: "failed" }));
    expect(findFailedJobByNormalizedUrl(NORM)?.jobId).toBe("failed");
    expect(findActiveJobByNormalizedUrl(NORM)).toBeUndefined();
    PressPodsJobEntity.delete({ jobId: "failed" });
  });
});

describe("reclaimProcessingJobsAtBoot", () => {
  beforeEach(() => {
    for (const jobId of ["p1", "q2"]) PressPodsJobEntity.delete({ jobId });
  });

  it("makes orphaned processing claims immediately reclaimable", () => {
    PressPodsJobEntity.upsert(
      job({ jobId: "p1", status: "processing", claimedAt: Date.now() }),
    );
    PressPodsJobEntity.upsert(job({ jobId: "q2", status: "queued" }));
    const count = reclaimProcessingJobsAtBoot();
    expect(count).toBe(1);
    expect(PressPodsJobEntity.get({ jobId: "p1" })?.claimedAt).toBe(0);
    // A queued job is untouched.
    expect(PressPodsJobEntity.get({ jobId: "q2" })?.status).toBe("queued");
    // The reclaimed claim is now selectable as stale.
    const p1 = PressPodsJobEntity.get({ jobId: "p1" });
    if (p1) expect(selectDueJobs([p1])).toHaveLength(1);
    for (const jobId of ["p1", "q2"]) PressPodsJobEntity.delete({ jobId });
  });
});

describe("deleteEpisodesByNormalizedUrlExcept", () => {
  const ep = (overrides: Partial<PressPodsEpisodeData>): PressPodsEpisodeData => ({
    episodeId: secureId(),
    title: "t",
    articleUrl: "https://replace.example/x",
    content: "c",
    audioFile: `${secureId()}.mp3`,
    fileBytes: 1,
    createdAt: NOW,
    ...overrides,
  });

  it("replaces older episodes sharing a canonical URL, keeping the newest", () => {
    const older = ep({
      createdAt: NOW - 1000,
      articleUrl: "https://replace.example/x?utm_source=a",
    });
    const newer = ep({
      createdAt: NOW,
      articleUrl: "https://replace.example/x?ref=b",
    });
    PressPodsEpisodeEntity.upsert(older);
    PressPodsEpisodeEntity.upsert(newer);

    const norm = "https://replace.example/x";
    const removed = deleteEpisodesByNormalizedUrlExcept(norm, newer.episodeId);
    expect(removed.map((r) => r.episodeId)).toEqual([older.episodeId]);
    expect(PressPodsEpisodeEntity.get({ episodeId: older.episodeId })).toBeUndefined();
    expect(PressPodsEpisodeEntity.get({ episodeId: newer.episodeId })).toBeDefined();

    PressPodsEpisodeEntity.delete({ episodeId: newer.episodeId });
  });
});
