import { Effect, Option } from "effect";
import { beforeEach, describe, expect, it } from "vitest";
import { runTest } from "../live-check/testRuntime.js";
import { PressPodsError } from "./effect.js";
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
  PressPodsPersistence,
  reclaimProcessingJobsAtBoot,
  recordJobFailure,
  requeueJobNow,
  retryDelayMs,
  STALE_CLAIM_MS,
  secureId,
  selectDueJobs,
} from "./persistence.js";

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
  beforeEach(async () => {
    await runTest(
      Effect.forEach(
        ["r1", "r2", "r3", "gone"],
        (jobId) => PressPodsJobEntity.delete({ jobId }),
        { discard: true },
      ),
    );
  });

  it("requeues a retryable failure with backoff", async () => {
    const updated = await runTest(recordJobFailure(job({ jobId: "r1" }), "boom", true));
    expect(updated.status).toBe("queued");
    expect(updated.attempts).toBe(1);
    expect(updated.nextAttemptAt).toBeGreaterThan(Date.now() - 1000);
    expect(updated.lastError).toBe("boom");
  });

  it("fails permanently on a non-retryable error", async () => {
    const updated = await runTest(
      recordJobFailure(job({ jobId: "r2" }), "bad article", false),
    );
    expect(updated.status).toBe("failed");
  });

  it("fails permanently once attempts are exhausted", async () => {
    const updated = await runTest(
      recordJobFailure(
        job({ jobId: "r3", attempts: MAX_JOB_ATTEMPTS - 1 }),
        "still broken",
        true,
      ),
    );
    expect(updated.status).toBe("failed");
    expect(updated.attempts).toBe(MAX_JOB_ATTEMPTS);
  });

  it("does not resurrect a concurrently-deleted job", async () => {
    const deleted = job({ jobId: "gone" });
    await runTest(recordJobFailure(deleted, "boom", true));
    expect(
      Option.getOrUndefined(await runTest(PressPodsJobEntity.get({ jobId: "gone" }))),
    ).toBeUndefined();
  });
});

describe("requeueJobNow", () => {
  it("requeues a failed job immediately", async () => {
    await runTest(PressPodsJobEntity.upsert(job({ jobId: "f1", status: "failed" })));
    const updated = await runTest(requeueJobNow("f1"));
    expect(updated?.status).toBe("queued");
    expect(updated?.nextAttemptAt).toBe(0);
    await runTest(PressPodsJobEntity.delete({ jobId: "f1" }));
  });

  it("refuses non-failed jobs", async () => {
    await runTest(
      PressPodsJobEntity.upsert(job({ jobId: "q1", status: "processing" })),
    );
    expect(await runTest(requeueJobNow("q1"))).toBeUndefined();
    await runTest(PressPodsJobEntity.delete({ jobId: "q1" }));
  });

  it("resets the attempt budget so an exhausted job gets a fresh retry cycle", async () => {
    await runTest(
      PressPodsJobEntity.upsert(
        job({
          jobId: "f2",
          status: "failed",
          attempts: MAX_JOB_ATTEMPTS,
          lastError: "boom",
        }),
      ),
    );
    const updated = await runTest(requeueJobNow("f2"));
    expect(updated?.attempts).toBe(0);
    expect(updated?.lastError).toBeUndefined();
    await runTest(PressPodsJobEntity.delete({ jobId: "f2" }));
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

  it("finds an episode created after the job was submitted", async () => {
    const row = episode({ createdAt: NOW });
    await runTest(PressPodsEpisodeEntity.upsert(row));
    expect(
      (await runTest(findEpisodeForJob(job({ createdAt: NOW - 1000 }))))?.episodeId,
    ).toBe(row.episodeId);
    await runTest(PressPodsEpisodeEntity.delete({ episodeId: row.episodeId }));
  });

  it("ignores older episodes for the same URL (resubmissions)", async () => {
    const row = episode({ createdAt: NOW - 60_000 });
    await runTest(PressPodsEpisodeEntity.upsert(row));
    expect(
      await runTest(findEpisodeForJob(job({ createdAt: NOW - 1000 }))),
    ).toBeUndefined();
    await runTest(PressPodsEpisodeEntity.delete({ episodeId: row.episodeId }));
  });

  it("matches on canonical identity despite tracking-param differences", async () => {
    const row = episode({
      createdAt: NOW,
      articleUrl: "https://example.com/story?utm_source=rss",
    });
    await runTest(PressPodsEpisodeEntity.upsert(row));
    const found = await runTest(
      findEpisodeForJob(
        job({ createdAt: NOW - 1000, url: "https://example.com/story?ref=twitter" }),
      ),
    );
    expect(found?.episodeId).toBe(row.episodeId);
    await runTest(PressPodsEpisodeEntity.delete({ episodeId: row.episodeId }));
  });
});

describe("PressPodsPersistence episode decoding", () => {
  const episode = (overrides: Partial<PressPodsEpisodeData>): PressPodsEpisodeData => ({
    episodeId: secureId(),
    title: "Persisted episode",
    articleUrl: "https://decode.example/article",
    content: "Narration",
    audioFile: "episode.mp3",
    fileBytes: 100,
    createdAt: NOW,
    ...overrides,
  });

  it("decodes every nested persisted diagnostic field", async () => {
    const row = episode({
      chapters: [{ startTimeSeconds: 1.5, title: "Opening" }],
      chunks: [
        {
          index: 0,
          sectionIndex: 1,
          sectionTitle: "Lead",
          text: "Narration",
          charCount: 9,
          durationSeconds: 2.5,
          startTimeSeconds: 1.5,
          secPerChar: 0.27,
          attempts: 2,
          coverage: 0.98,
          wordRatio: 1,
          expectedWords: 1,
          resplit: true,
          resplitDepth: 1,
        },
      ],
      retrieverAttempts: [
        { name: "readability", success: true, contentRating: 9, textChars: 1000 },
        { name: "fetch", success: false, error: "HTTP 500" },
      ],
      costs: {
        llmCents: 1.2,
        ttsCents: 3.4,
        detailCents: { metadata: 1.2 },
        detailTokens: { metadata: { input: 100, output: 20 } },
        detailChars: { speech: 1000 },
      },
    });
    await runTest(PressPodsEpisodeEntity.upsert(row));
    try {
      await expect(
        runTest(PressPodsPersistence.getEpisode(row.episodeId)),
      ).resolves.toEqual(row);
    } finally {
      await runTest(PressPodsEpisodeEntity.delete({ episodeId: row.episodeId }));
    }
  });

  it.each([
    ["chapters", [{ startTimeSeconds: "soon", title: "Opening" }]],
    [
      "chunks",
      [
        {
          index: 0,
          sectionIndex: 0,
          charCount: 9,
          durationSeconds: 2,
          startTimeSeconds: 0,
          secPerChar: 0.2,
          attempts: 1,
        },
      ],
    ],
    ["retrieverAttempts", [{ name: "fetch", success: true, textChars: 100 }]],
    [
      "costs",
      {
        llmCents: 1,
        ttsCents: 2,
        detailCents: {},
        detailTokens: { metadata: { input: 10, output: "twenty" } },
        detailChars: {},
      },
    ],
  ])("rejects malformed persisted %s", async (field, malformed) => {
    const row = episode({
      episodeId: `malformed-${field}`,
      [field]: malformed,
    } as unknown as Partial<PressPodsEpisodeData>);
    await runTest(PressPodsEpisodeEntity.upsert(row));
    try {
      const error = await runTest(
        Effect.flip(PressPodsPersistence.getEpisode(row.episodeId)),
      );
      expect(error).toBeInstanceOf(PressPodsError);
      if (error instanceof PressPodsError) {
        expect(error.operation).toBe("decode PressPods episode");
      }
    } finally {
      await runTest(PressPodsEpisodeEntity.delete({ episodeId: row.episodeId }));
    }
  });
});

describe("URL-based dedup lookups", () => {
  const URL_A = "https://dedup.example/piece?utm_source=x";
  const NORM = "https://dedup.example/piece";

  beforeEach(async () => {
    await runTest(
      Effect.forEach(
        ["active", "failed"],
        (jobId) => PressPodsJobEntity.delete({ jobId }),
        { discard: true },
      ),
    );
  });

  it("finds a queued or processing job by canonical URL", async () => {
    await runTest(
      PressPodsJobEntity.upsert(
        job({ jobId: "active", url: URL_A, status: "processing" }),
      ),
    );
    expect((await runTest(findActiveJobByNormalizedUrl(NORM)))?.jobId).toBe("active");
    expect(await runTest(findFailedJobByNormalizedUrl(NORM))).toBeUndefined();
    await runTest(PressPodsJobEntity.delete({ jobId: "active" }));
  });

  it("finds a failed job by canonical URL", async () => {
    await runTest(
      PressPodsJobEntity.upsert(job({ jobId: "failed", url: URL_A, status: "failed" })),
    );
    expect((await runTest(findFailedJobByNormalizedUrl(NORM)))?.jobId).toBe("failed");
    expect(await runTest(findActiveJobByNormalizedUrl(NORM))).toBeUndefined();
    await runTest(PressPodsJobEntity.delete({ jobId: "failed" }));
  });
});

describe("reclaimProcessingJobsAtBoot", () => {
  beforeEach(async () => {
    await runTest(
      Effect.forEach(["p1", "q2"], (jobId) => PressPodsJobEntity.delete({ jobId }), {
        discard: true,
      }),
    );
  });

  it("makes orphaned processing claims immediately reclaimable", async () => {
    await runTest(
      PressPodsJobEntity.upsert(
        job({ jobId: "p1", status: "processing", claimedAt: Date.now() }),
      ),
    );
    await runTest(PressPodsJobEntity.upsert(job({ jobId: "q2", status: "queued" })));
    const count = await runTest(reclaimProcessingJobsAtBoot());
    expect(count).toBe(1);
    expect(
      Option.getOrUndefined(await runTest(PressPodsJobEntity.get({ jobId: "p1" })))
        ?.claimedAt,
    ).toBe(0);
    // A queued job is untouched.
    expect(
      Option.getOrUndefined(await runTest(PressPodsJobEntity.get({ jobId: "q2" })))
        ?.status,
    ).toBe("queued");
    // The reclaimed claim is now selectable as stale.
    const p1 = Option.getOrUndefined(
      await runTest(PressPodsJobEntity.get({ jobId: "p1" })),
    );
    if (p1) expect(selectDueJobs([p1])).toHaveLength(1);
    await runTest(
      Effect.forEach(["p1", "q2"], (jobId) => PressPodsJobEntity.delete({ jobId }), {
        discard: true,
      }),
    );
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

  it("replaces older episodes sharing a canonical URL, keeping the newest", async () => {
    const older = ep({
      createdAt: NOW - 1000,
      articleUrl: "https://replace.example/x?utm_source=a",
    });
    const newer = ep({
      createdAt: NOW,
      articleUrl: "https://replace.example/x?ref=b",
    });
    await runTest(PressPodsEpisodeEntity.upsert(older));
    await runTest(PressPodsEpisodeEntity.upsert(newer));

    const norm = "https://replace.example/x";
    const removed = await runTest(
      deleteEpisodesByNormalizedUrlExcept(norm, newer.episodeId),
    );
    expect(removed.map((r) => r.episodeId)).toEqual([older.episodeId]);
    expect(
      Option.getOrUndefined(
        await runTest(PressPodsEpisodeEntity.get({ episodeId: older.episodeId })),
      ),
    ).toBeUndefined();
    expect(
      Option.getOrUndefined(
        await runTest(PressPodsEpisodeEntity.get({ episodeId: newer.episodeId })),
      ),
    ).toBeDefined();

    await runTest(PressPodsEpisodeEntity.delete({ episodeId: newer.episodeId }));
  });
});
