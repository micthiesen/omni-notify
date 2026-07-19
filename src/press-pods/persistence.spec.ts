import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it } from "vitest";
import {
  findEpisodeForJob,
  MAX_JOB_ATTEMPTS,
  type PressPodsEpisodeData,
  PressPodsEpisodeEntity,
  type PressPodsJobData,
  PressPodsJobEntity,
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
});
