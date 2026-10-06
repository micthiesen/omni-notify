import { z } from "zod";

export const pageSchema = z.object({
  nextCursor: z.number().int().nonnegative().nullable(),
  total: z.number().int().nonnegative(),
});

export const claimSchema = z.object({
  claim: z.string(),
  confidence: z.number(),
  evidenceIds: z.array(z.string()),
});

export const sourcePerformanceSchema = z.record(
  z.string(),
  z.object({
    total: z.number().int().nonnegative(),
    watched: z.number().int().nonnegative(),
    goodPick: z.number().int().nonnegative(),
    notForMe: z.number().int().nonnegative(),
  }),
);

export const profileStatsSchema = z.union([
  z.object({
    completedMovies: z.number().int().nonnegative(),
    completedSeries: z.number().int().nonnegative(),
    rewatchedTitles: z.number().int().nonnegative(),
    recommendations: z.object({
      total: z.number().int().nonnegative(),
      watched: z.number().int().nonnegative(),
      abandoned: z.number().int().nonnegative(),
      ignored: z.number().int().nonnegative(),
      failed: z.number().int().nonnegative(),
      awaitingOutcome: z.number().int().nonnegative(),
    }),
    feedback: z.object({
      goodPick: z.number().int().nonnegative(),
      notForMe: z.number().int().nonnegative(),
      alreadyWatched: z.number().int().nonnegative(),
    }),
    averageHoursToStart: z.number().optional(),
    sourcePerformance: sourcePerformanceSchema,
  }),
  z.object({
    listenedEpisodes: z.number().int().nonnegative(),
    startedEpisodes: z.number().int().nonnegative(),
    starredEpisodes: z.number().int().nonnegative(),
    distinctShows: z.number().int().nonnegative(),
    recommendations: z.object({
      total: z.number().int().nonnegative(),
      listened: z.number().int().nonnegative(),
      abandoned: z.number().int().nonnegative(),
      ignored: z.number().int().nonnegative(),
      failed: z.number().int().nonnegative(),
      awaitingOutcome: z.number().int().nonnegative(),
    }),
    feedback: z.object({
      goodPick: z.number().int().nonnegative(),
      notForMe: z.number().int().nonnegative(),
    }),
  }),
]);

export const profileSchema = z
  .object({
    profileId: z.string(),
    version: z.number().int(),
    generatedAt: z.number().int(),
    evidenceFingerprint: z.string(),
    evidenceCount: z.number().int().nonnegative(),
    modelId: z.string(),
    promptVersion: z.string(),
    summary: z.string(),
    stablePreferences: z.array(claimSchema),
    conditionalPreferences: z.array(claimSchema),
    aversions: z.array(claimSchema),
    currentSaturation: z.array(claimSchema),
    explorationTargets: z.array(claimSchema),
    uncertainties: z.array(claimSchema),
    stats: profileStatsSchema,
    commitmentPreferences: z
      .object({
        movies: z.object({
          preference: z.enum(["positive", "neutral", "negative", "uncertain"]),
          confidence: z.number(),
          evidenceIds: z.array(z.string()),
        }),
        limitedSeries: z.object({
          preference: z.enum(["positive", "neutral", "negative", "uncertain"]),
          confidence: z.number(),
          evidenceIds: z.array(z.string()),
        }),
        longSeries: z.object({
          preference: z.enum(["positive", "neutral", "negative", "uncertain"]),
          confidence: z.number(),
          evidenceIds: z.array(z.string()),
        }),
      })
      .optional(),
  })
  .strict();

export function requireAvailable<T>(
  result: { status: "ok"; value: T } | { status: "unavailable"; reason: string },
): T {
  if (result.status !== "ok") throw new Error(result.reason);
  return result.value;
}
