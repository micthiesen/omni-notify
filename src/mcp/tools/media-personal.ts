import { Clock, Effect } from "effect";
import { z } from "zod";
import { CostEventEntity, getCostEvents } from "../../costs/persistence.js";
import { summarizeCosts } from "../../costs/summary.js";
import {
  getAllPetsWithHistory,
  getDailyVisitCounts,
  getPet,
  getWeightHistory,
} from "../../pet-tracker/persistence.js";
import {
  type EnqueueEpisodeRequest,
  PodcastQueuePosition,
  type PodcastWriteResult,
  resolvePodcastAccountEffect,
} from "../../podcast-recs/account.js";
import {
  getAllPodcastRecommendations,
  getPodcastRecommendation,
  setPodcastRecommendationFeedback,
} from "../../podcast-recs/persistence.js";
import {
  getAllPodcastTasteEvidence,
  getLatestPodcastTasteProfile,
} from "../../podcast-recs/reflection/persistence.js";
import {
  jobNormalizedUrl,
  PressPodsPersistence,
  type PressPodsEpisodeData,
  type PressPodsJobData,
} from "../../press-pods/persistence.js";
import { assertPublicHttpUrl } from "../../press-pods/publicHttp.js";
import {
  checkpointWorkId,
  clearChunkCheckpoints,
  deleteEpisodeAudio,
} from "../../press-pods/storage.js";
import {
  submitEpisodeSchema,
  submitEpisodeUrlEffect,
} from "../../press-pods/submit.js";
import {
  fetchInProgressEffect,
  fetchLibraryIndexEffect,
  fetchWatchHistoryEffect,
} from "../../recommendations/mediaLibrary.js";
import {
  getAllRecommendations,
  getRecommendation,
  setRecommendationFeedback,
} from "../../recommendations/persistence.js";
import {
  getAllTasteEvidence,
  getLatestTasteProfile,
} from "../../recommendations/taste/persistence.js";
import {
  discoverTitlesEffect,
  fetchTitleDetailsEffect,
  fetchTrendingEffect,
  getTmdbUrl,
  searchTitlesEffect,
} from "../../recommendations/tmdb/client.js";
import {
  type InProgressItem,
  type MediaItem,
  MediaType,
  type WatchedItem,
} from "../../recommendations/types.js";
import {
  addToWatchlistEffect,
  fetchWatchlistEffect,
} from "../../recommendations/watchlist.js";
import {
  TaskAlreadyRunningError,
  TaskNotFoundError,
} from "../../task-runs/registry.js";
import config from "../../utils/config.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
  truncate,
} from "../tool.js";

const nullableString = z.string().nullable();
const nullableNumber = z.number().nullable();
const MAX_MCP_COST_EVENTS = 100_000;
const mediaTypeSchema = z.enum([MediaType.Movie, MediaType.Tv]);
const pageSchema = z.object({
  nextCursor: z.number().int().nonnegative().nullable(),
  total: z.number().int().nonnegative(),
});

const tmdbTitleSchema = z.object({
  tmdbId: z.number().int().positive(),
  mediaType: mediaTypeSchema,
  title: z.string(),
  year: z.number().int().nullable(),
  overview: z.string(),
  genreIds: z.array(z.number().int()),
  voteAverage: z.number(),
  voteCount: z.number().int().nonnegative(),
  popularity: z.number(),
  posterPath: nullableString,
  originalLanguage: nullableString,
  tmdbUrl: z.string().url(),
});

const mediaItemSchema = z.object({
  guid: z.string(),
  title: z.string(),
  year: z.number().int().nullable(),
  mediaType: mediaTypeSchema,
  externalIds: z
    .object({
      tmdb: z.number().int().optional(),
      imdb: z.string().optional(),
      tvdb: z.number().int().optional(),
    })
    .nullable(),
  progress: z.number().min(0).max(1).optional(),
  lastViewedAt: z.number().int().optional(),
  viewedAt: z.number().int().optional(),
  viewCount: z.number().int().nonnegative().optional(),
  completion: z.number().min(0).max(1).optional(),
});

const recommendationSchema = z.object({
  recommendationId: z.string(),
  canonicalId: z.string(),
  tmdbId: z.number().int().positive(),
  mediaType: mediaTypeSchema,
  title: z.string(),
  year: z.number().int().nullable(),
  status: z.enum(["pending", "notified", "watched", "abandoned", "ignored", "failed"]),
  whyForUser: nullableString,
  caveats: z.array(z.string()),
  confidence: nullableNumber,
  genres: z.array(z.string()),
  runtimeMinutes: nullableNumber,
  seasonCount: nullableNumber,
  episodeCount: nullableNumber,
  runDate: z.string(),
  recommendedAt: z.number().int(),
  notifiedAt: nullableNumber,
  resolvedAt: nullableNumber,
  watchlistResult: nullableString,
  feedback: z.enum(["good_pick", "not_for_me", "already_watched"]).nullable(),
  feedbackAt: nullableNumber,
  feedbackNote: nullableString,
});

const podcastRecommendationSchema = z.object({
  recommendationId: z.string(),
  episodeId: z.string(),
  showId: z.string(),
  showTitle: z.string(),
  episodeTitle: z.string(),
  feedUrl: z.string().url(),
  itunesId: z.number().int().nullable(),
  episodeGuid: z.string(),
  mediaUrl: nullableString,
  episodeUrl: nullableString,
  publishedAt: z.number().int(),
  durationMinutes: nullableNumber,
  status: z.enum(["pending", "notified", "listened", "abandoned", "ignored", "failed"]),
  whyForUser: nullableString,
  caveats: z.array(z.string()),
  confidence: nullableNumber,
  discoveredVia: nullableString,
  matchedVoices: z.array(z.string()),
  recommendedAt: z.number().int(),
  notifiedAt: nullableNumber,
  resolvedAt: nullableNumber,
  queueResult: nullableString,
  feedback: z.enum(["good_pick", "not_for_me"]).nullable(),
  feedbackAt: nullableNumber,
  feedbackNote: nullableString,
});

const claimSchema = z.object({
  claim: z.string(),
  confidence: z.number(),
  evidenceIds: z.array(z.string()),
});

const sourcePerformanceSchema = z.record(
  z.string(),
  z.object({
    total: z.number().int().nonnegative(),
    watched: z.number().int().nonnegative(),
    goodPick: z.number().int().nonnegative(),
    notForMe: z.number().int().nonnegative(),
  }),
);

const profileStatsSchema = z.union([
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

const profileSchema = z
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

const podcastSubscriptionSchema = z.object({
  title: z.string(),
  feedUrl: z.string().optional(),
  itunesId: z.number().int().optional(),
});
const queuedEpisodeSchema = z.object({
  showTitle: z.string(),
  episodeTitle: z.string(),
  episodeGuid: z.string().optional(),
  feedUrl: z.string().optional(),
  description: z.string().optional(),
  addedAt: z.number().int().optional(),
});
const inboxEpisodeSchema = z.object({
  clientEpisodeId: z.string(),
  showTitle: z.string(),
  episodeTitle: z.string(),
  episodeGuid: z.string().optional(),
  description: z.string().optional(),
});
const listenedEpisodeSchema = z.object({
  showTitle: z.string(),
  episodeTitle: z.string(),
  episodeGuid: z.string().optional(),
  feedUrl: z.string().optional(),
  itunesId: z.number().int().optional(),
  listenedAt: z.number().int(),
  completion: z.number().min(0).max(1).optional(),
  starred: z.boolean().optional(),
});
const podcastSearchResultSchema = z.object({
  clientId: z.string(),
  title: z.string(),
  author: z.string().optional(),
  feedUrl: z.string(),
  itunesId: z.number().int().optional(),
  summary: z.string().optional(),
  artworkUrl: z.string().optional(),
});
const episodeSearchResultSchema = z.object({
  clientId: z.string(),
  title: z.string(),
  showTitle: z.string(),
  author: z.string().optional(),
  publishedAt: z.number().int().optional(),
  artworkUrl: z.string().optional(),
});

const costUsageSchema = z.object({
  inputTokens: z.number().nonnegative().optional(),
  inputNoCacheTokens: z.number().nonnegative().optional(),
  cacheReadTokens: z.number().nonnegative().optional(),
  cacheWriteTokens: z.number().nonnegative().optional(),
  outputTokens: z.number().nonnegative().optional(),
  reasoningTokens: z.number().nonnegative().optional(),
  characters: z.number().nonnegative().optional(),
  requests: z.number().nonnegative().optional(),
  credits: z.number().nonnegative().optional(),
});

const pressPodsEpisodeSchema = z.object({
  episodeId: z.string(),
  title: z.string(),
  author: nullableString,
  publication: nullableString,
  domain: nullableString,
  articleUrl: z.string().url(),
  excerpt: nullableString,
  voiceName: nullableString,
  voiceProvider: nullableString,
  durationSeconds: nullableNumber,
  fileBytes: z.number().int().nonnegative(),
  retrieverName: nullableString,
  costCents: nullableNumber,
  createdAt: z.number().int(),
  publishedAt: nullableNumber,
  runId: nullableString,
  chapterCount: z.number().int().nonnegative(),
});

const pressPodsJobSchema = z.object({
  jobId: z.string(),
  url: z.string().url(),
  status: z.enum(["queued", "processing", "failed"]),
  attempts: z.number().int().nonnegative(),
  nextAttemptAt: nullableNumber,
  lastError: nullableString,
  createdAt: z.number().int(),
  updatedAt: z.number().int(),
  lastRunId: nullableString,
});

function serializeTmdbTitle(
  title: Effect.Success<ReturnType<typeof searchTitlesEffect>>[number],
) {
  return {
    ...title,
    year: title.year ?? null,
    posterPath: title.posterPath ?? null,
    originalLanguage: title.originalLanguage ?? null,
    tmdbUrl: getTmdbUrl(title.mediaType, title.tmdbId),
  };
}

function serializeMediaItem(item: MediaItem | InProgressItem | WatchedItem) {
  return {
    guid: item.guid,
    title: item.title,
    year: item.year ?? null,
    mediaType: item.mediaType,
    externalIds: item.externalIds ?? null,
    ...("progress" in item ? { progress: item.progress } : {}),
    ...("lastViewedAt" in item ? { lastViewedAt: item.lastViewedAt } : {}),
    ...("viewedAt" in item ? { viewedAt: item.viewedAt } : {}),
    ...("viewCount" in item ? { viewCount: item.viewCount } : {}),
    ...("completion" in item && item.completion !== undefined
      ? { completion: item.completion }
      : {}),
  };
}

function serializeRecommendation(
  rec: ReturnType<typeof getAllRecommendations>[number],
) {
  return {
    recommendationId: rec.recommendationId,
    canonicalId: rec.canonicalId,
    tmdbId: rec.tmdbId,
    mediaType: rec.mediaType,
    title: rec.title,
    year: rec.year ?? null,
    status: rec.status,
    whyForUser: rec.whyForUser ?? null,
    caveats: rec.caveats ?? [],
    confidence: rec.confidence ?? null,
    genres: rec.genres ?? [],
    runtimeMinutes: rec.runtimeMinutes ?? null,
    seasonCount: rec.seasonCount ?? null,
    episodeCount: rec.episodeCount ?? null,
    runDate: rec.runDate,
    recommendedAt: rec.recommendedAt,
    notifiedAt: rec.notifiedAt ?? null,
    resolvedAt: rec.resolvedAt ?? null,
    watchlistResult: rec.watchlistResult ?? null,
    feedback: rec.feedback ?? null,
    feedbackAt: rec.feedbackAt ?? null,
    feedbackNote: rec.feedbackNote ?? null,
  };
}

function serializePodcastRecommendation(
  rec: ReturnType<typeof getAllPodcastRecommendations>[number],
) {
  return {
    recommendationId: rec.recommendationId,
    episodeId: rec.episodeId,
    showId: rec.showId,
    showTitle: rec.showTitle,
    episodeTitle: rec.episodeTitle,
    feedUrl: rec.feedUrl,
    itunesId: rec.itunesId ?? null,
    episodeGuid: rec.episodeGuid,
    mediaUrl: rec.mediaUrl ?? null,
    episodeUrl: rec.episodeUrl ?? null,
    publishedAt: rec.publishedAt,
    durationMinutes: rec.durationMinutes ?? null,
    status: rec.status,
    whyForUser: rec.whyForUser ?? null,
    caveats: rec.caveats ?? [],
    confidence: rec.confidence ?? null,
    discoveredVia: rec.discoveredVia ?? null,
    matchedVoices: rec.matchedVoices ?? [],
    recommendedAt: rec.recommendedAt,
    notifiedAt: rec.notifiedAt ?? null,
    resolvedAt: rec.resolvedAt ?? null,
    queueResult: rec.queueResult ?? null,
    feedback: rec.feedback ?? null,
    feedbackAt: rec.feedbackAt ?? null,
    feedbackNote: rec.feedbackNote ?? null,
  };
}

function serializeEpisode(episode: PressPodsEpisodeData) {
  return {
    episodeId: episode.episodeId,
    title: episode.title,
    author: episode.author ?? null,
    publication: episode.publication ?? null,
    domain: episode.domain ?? null,
    articleUrl: episode.articleUrl,
    excerpt: episode.excerpt ?? null,
    voiceName: episode.voiceName ?? null,
    voiceProvider: episode.voiceProvider ?? null,
    durationSeconds: episode.durationSeconds ?? null,
    fileBytes: episode.fileBytes,
    retrieverName: episode.retrieverName ?? null,
    costCents: episode.costs
      ? Math.round((episode.costs.llmCents + episode.costs.ttsCents) * 100) / 100
      : null,
    createdAt: episode.createdAt,
    publishedAt: episode.publishedAt ?? null,
    runId: episode.runId ?? null,
    chapterCount: episode.chapters?.length ?? 0,
  };
}

function serializeJob(job: PressPodsJobData) {
  return {
    jobId: job.jobId,
    url: job.url,
    status: job.status,
    attempts: job.attempts,
    nextAttemptAt: job.nextAttemptAt || null,
    lastError: job.lastError ?? null,
    createdAt: job.createdAt,
    updatedAt: job.updatedAt,
    lastRunId: job.lastRunId ?? null,
  };
}

function requireAvailable<T>(
  result: { status: "ok"; value: T } | { status: "unavailable"; reason: string },
): T {
  if (result.status !== "ok") throw new Error(result.reason);
  return result.value;
}

function kickPressPods(runtime: McpRuntime): void {
  try {
    runtime.registry.runNow("PressPods");
  } catch (error) {
    if (
      !(error instanceof TaskAlreadyRunningError) &&
      !(error instanceof TaskNotFoundError)
    ) {
      throw error;
    }
  }
}

const requirePodcastAccountEffect = Effect.fn("McpMedia.requirePodcastAccount")(
  function* (runtime: McpRuntime) {
    const account =
      runtime.podcastAccount ?? (yield* resolvePodcastAccountEffect(runtime.logger));
    if (!account) {
      return yield* Effect.fail(new Error("Podcast account is not configured"));
    }
    return account;
  },
);

function matchesQuery<
  T extends { title?: string; showTitle?: string; episodeTitle?: string },
>(values: T[], query: string | undefined): T[] {
  if (!query) return values;
  const needle = query.toLocaleLowerCase();
  return values.filter((value) =>
    [value.title, value.showTitle, value.episodeTitle].some((text) =>
      text?.toLocaleLowerCase().includes(needle),
    ),
  );
}

export function createMediaPersonalTools(runtime: McpRuntime): McpToolDefinition[] {
  const tools: McpToolDefinition[] = [];

  tools.push(
    defineTool({
      name: "media_catalog_search",
      title: "Search Media Catalog",
      description:
        "Search TMDB for movies or television series. This consumes the configured TMDB API quota but does not change any account.",
      inputSchema: z
        .object({
          query: z.string().trim().min(1).max(200),
          mediaType: mediaTypeSchema,
          year: z.number().int().min(1888).max(2200).optional(),
          cursor: z.number().int().min(0).default(0),
          limit: z.number().int().min(1).max(20).default(10),
        })
        .strict(),
      outputSchema: pageSchema.extend({ items: z.array(tmdbTitleSchema) }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads the public TMDB catalog"],
        cost: "Consumes a small amount of the configured TMDB API quota; no per-call purchase",
        recommendedPolicy: "allow",
      },
      execute: ({ query, mediaType, year, cursor, limit }) =>
        Effect.gen(function* () {
          const values = (yield* searchTitlesEffect(query, mediaType, year)).map(
            serializeTmdbTitle,
          );
          return paginate(values, cursor, limit);
        }),
    }),
    defineTool({
      name: "media_catalog_get",
      title: "Get Media Catalog Details",
      description:
        "Get bounded TMDB metadata for one movie or television series, including cast, creators, keywords, certification, and viewing commitment.",
      inputSchema: z
        .object({ mediaType: mediaTypeSchema, tmdbId: z.number().int().positive() })
        .strict(),
      outputSchema: z.object({
        mediaType: mediaTypeSchema,
        tmdbId: z.number().int().positive(),
        tmdbUrl: z.string().url(),
        details: z.object({
          genres: z.array(z.string()),
          runtimeMinutes: nullableNumber,
          seasonCount: nullableNumber,
          episodeCount: nullableNumber,
          seriesStatus: nullableString,
          originalLanguage: nullableString,
          originCountries: z.array(z.string()),
          creators: z.array(z.string()),
          cast: z.array(z.string()),
          keywords: z.array(z.string()),
          certification: nullableString,
        }),
      }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads the public TMDB catalog"],
        cost: "Consumes a small amount of the configured TMDB API quota; no per-call purchase",
        recommendedPolicy: "allow",
      },
      execute: ({ mediaType, tmdbId }) =>
        Effect.gen(function* () {
          const details = yield* fetchTitleDetailsEffect(mediaType, tmdbId);
          return {
            mediaType,
            tmdbId,
            tmdbUrl: getTmdbUrl(mediaType, tmdbId),
            details: {
              ...details,
              runtimeMinutes: details.runtimeMinutes ?? null,
              seasonCount: details.seasonCount ?? null,
              episodeCount: details.episodeCount ?? null,
              seriesStatus: details.seriesStatus ?? null,
              originalLanguage: details.originalLanguage ?? null,
              certification: details.certification ?? null,
            },
          };
        }),
    }),
    defineTool({
      name: "media_catalog_browse",
      title: "Browse Media Catalog",
      description:
        "Browse weekly trending titles or a filtered TMDB discovery page. Results are bounded and adult titles are excluded by the underlying client.",
      inputSchema: z.discriminatedUnion("mode", [
        z.object({
          mode: z.literal("trending"),
          cursor: z.number().int().min(0).default(0),
          limit: z.number().int().min(1).max(20).default(10),
        }),
        z.object({
          mode: z.literal("discover"),
          mediaType: mediaTypeSchema,
          withGenres: z.array(z.number().int().positive()).max(10).optional(),
          withoutGenres: z.array(z.number().int().positive()).max(10).optional(),
          originalLanguage: z
            .string()
            .regex(/^[a-z]{2}$/)
            .optional(),
          minVoteCount: z.number().int().min(0).max(100_000).default(300),
          page: z.number().int().min(1).max(500).default(1),
          cursor: z.number().int().min(0).default(0),
          limit: z.number().int().min(1).max(20).default(10),
        }),
      ]),
      outputSchema: pageSchema.extend({ items: z.array(tmdbTitleSchema) }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads the public TMDB catalog"],
        cost: "Consumes a small amount of the configured TMDB API quota; no per-call purchase",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const values = (
            input.mode === "trending"
              ? yield* fetchTrendingEffect()
              : yield* discoverTitlesEffect(input.mediaType, {
                  withGenres: input.withGenres,
                  withoutGenres: input.withoutGenres,
                  withOriginalLanguage: input.originalLanguage,
                  minVoteCount: input.minVoteCount,
                  page: input.page,
                })
          ).map(serializeTmdbTitle);
          return paginate(values, input.cursor, input.limit);
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "media_library_list",
      title: "List Plex Media",
      description:
        "List the configured Plex library, recent watch history, or in-progress items. An unavailable Plex server is reported as an error, never as an empty library.",
      inputSchema: z
        .object({
          view: z.enum(["library", "history", "in_progress"]),
          mediaType: mediaTypeSchema.optional(),
          query: z.string().trim().max(200).optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: pageSchema.extend({ items: z.array(mediaItemSchema) }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads the configured Plex account and server"],
        cost: "No expected monetary cost; bounded account API traffic",
        recommendedPolicy: "allow",
      },
      execute: ({ view, mediaType, query, cursor, limit }) =>
        Effect.gen(function* () {
          const result =
            view === "history"
              ? yield* fetchWatchHistoryEffect()
              : view === "in_progress"
                ? yield* fetchInProgressEffect()
                : yield* fetchLibraryIndexEffect();
          let values = requireAvailable(result).map((item) => serializeMediaItem(item));
          if (mediaType) values = values.filter((item) => item.mediaType === mediaType);
          if (query) {
            const needle = query.toLocaleLowerCase();
            values = values.filter((item) =>
              String(item.title).toLocaleLowerCase().includes(needle),
            );
          }
          return paginate(values, cursor, limit);
        }),
    }),
    defineTool({
      name: "media_watchlist_list",
      title: "List Managed Watchlist",
      description:
        "List titles tracked by the configured Radarr and Sonarr services. Either service being unavailable is reported as an error to avoid returning partial state.",
      inputSchema: z
        .object({
          mediaType: mediaTypeSchema.optional(),
          query: z.string().trim().max(200).optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: pageSchema.extend({ items: z.array(mediaItemSchema) }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads the configured Radarr and Sonarr accounts"],
        cost: "No expected monetary cost; bounded account API traffic",
        recommendedPolicy: "allow",
      },
      execute: ({ mediaType, query, cursor, limit }) =>
        Effect.gen(function* () {
          let values = requireAvailable(yield* fetchWatchlistEffect()).map((item) =>
            serializeMediaItem(item),
          );
          if (mediaType) values = values.filter((item) => item.mediaType === mediaType);
          if (query) {
            const needle = query.toLocaleLowerCase();
            values = values.filter((item) =>
              String(item.title).toLocaleLowerCase().includes(needle),
            );
          }
          return paginate(values, cursor, limit);
        }),
    }),
    defineTool({
      name: "media_watchlist_add",
      title: "Add to Managed Watchlist",
      description:
        "Add a TMDB movie to Radarr or a TMDB series to Sonarr. This can begin acquisition and downloads on managed services, so Executor approval is required.",
      inputSchema: z
        .object({
          tmdbId: z.number().int().positive(),
          mediaType: mediaTypeSchema,
          title: z.string().trim().min(1).max(300),
          year: z.number().int().min(1888).max(2200).optional(),
        })
        .strict(),
      outputSchema: z.object({
        result: z.enum([
          "added",
          "already_exists",
          "not_found",
          "unavailable",
          "error",
        ]),
        titleSlug: nullableString,
      }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: [
          "Changes the configured Radarr or Sonarr account",
          "May begin media acquisition and downloads",
        ],
        cost: "No direct API charge; may consume storage, bandwidth, and provider resources",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const outcome = yield* addToWatchlistEffect(input);
          return { result: outcome.result, titleSlug: outcome.titleSlug ?? null };
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "media_recommendations_list",
      title: "List Media Recommendations",
      description:
        "List persisted media recommendation attempts and outcomes, optionally filtered by status or feedback.",
      inputSchema: z
        .object({
          status: z
            .enum(["pending", "notified", "watched", "abandoned", "ignored", "failed"])
            .optional(),
          feedback: z
            .enum(["good_pick", "not_for_me", "already_watched", "none"])
            .optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: pageSchema.extend({ items: z.array(recommendationSchema) }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ status, feedback, cursor, limit }) =>
        Effect.sync(() => {
          let values = getAllRecommendations();
          if (status) values = values.filter((item) => item.status === status);
          if (feedback) {
            values = values.filter((item) =>
              feedback === "none" ? !item.feedback : item.feedback === feedback,
            );
          }
          return paginate(values.map(serializeRecommendation), cursor, limit);
        }),
    }),
    defineTool({
      name: "media_recommendation_get",
      title: "Get Media Recommendation",
      description: "Get one persisted media recommendation by its recommendation ID.",
      inputSchema: z.object({ recommendationId: z.string().min(1).max(200) }).strict(),
      outputSchema: z.object({ recommendation: recommendationSchema }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ recommendationId }) =>
        Effect.sync(() => {
          const value = getRecommendation(recommendationId);
          if (!value) throw new Error("Media recommendation not found");
          return { recommendation: serializeRecommendation(value) };
        }),
    }),
    defineTool({
      name: "media_recommendation_feedback",
      title: "Record Media Recommendation Feedback",
      description:
        "Record a good-pick, not-for-me, or already-watched assessment and/or a bounded note. This changes only Omni's local recommendation state.",
      inputSchema: z
        .object({
          recommendationId: z.string().min(1).max(200),
          feedback: z.enum(["good_pick", "not_for_me", "already_watched"]).optional(),
          note: z.string().trim().max(1000).optional(),
        })
        .strict()
        .refine((value) => value.feedback !== undefined || value.note !== undefined, {
          message: "feedback or note is required",
        }),
      outputSchema: z.object({ recommendation: recommendationSchema }),
      annotations: annotations(false, false, false, false),
      policy: {
        sideEffects: [
          "Updates local recommendation feedback used by future taste analysis",
        ],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ recommendationId, feedback, note }) =>
        Effect.sync(() => {
          const value = setRecommendationFeedback(recommendationId, { feedback, note });
          if (!value) throw new Error("Media recommendation not found");
          return { recommendation: serializeRecommendation(value) };
        }),
    }),
    defineTool({
      name: "media_taste_read",
      title: "Read Media Taste Data",
      description:
        "Read the latest derived media taste profile or paginated evidence rows supporting it.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({ resource: z.literal("profile") }),
        z.object({
          resource: z.literal("evidence"),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        }),
      ]),
      outputSchema: z.discriminatedUnion("resource", [
        z.object({ resource: z.literal("profile"), profile: profileSchema.nullable() }),
        pageSchema.extend({
          resource: z.literal("evidence"),
          items: z.array(
            z.object({
              evidenceId: z.string(),
              kind: z.enum([
                "plex_watch",
                "recommendation_outcome",
                "explicit_feedback",
              ]),
              canonicalId: z.string(),
              title: z.string(),
              mediaType: mediaTypeSchema,
              observedAt: z.number().int(),
              completion: nullableNumber,
              recommendationId: nullableString,
              feedback: z
                .enum(["good_pick", "not_for_me", "already_watched"])
                .nullable(),
              note: nullableString,
            }),
          ),
        }),
      ]),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.sync(() => {
          if (input.resource === "profile") {
            return { resource: "profile", profile: getLatestTasteProfile() ?? null };
          }
          const values = getAllTasteEvidence().map((item) => ({
            evidenceId: item.evidenceId,
            kind: item.kind,
            canonicalId: item.canonicalId,
            title: item.title,
            mediaType: item.mediaType,
            observedAt: item.observedAt,
            completion: item.completion ?? null,
            recommendationId: item.recommendationId ?? null,
            feedback: item.feedback ?? null,
            note: item.note ?? null,
          }));
          return {
            resource: "evidence",
            ...paginate(values, input.cursor, input.limit),
          };
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "podcast_account_list",
      title: "List Podcast Account Resources",
      description:
        "List bounded subscriptions, queue, inbox, or recent listening history from the configured podcast account. Unavailable account state is reported as an error.",
      inputSchema: z
        .object({
          resource: z.enum(["subscriptions", "queue", "inbox", "listen_history"]),
          sinceDays: z.number().int().min(1).max(180).default(30),
          query: z.string().trim().max(200).optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: z.discriminatedUnion("resource", [
        pageSchema.extend({
          account: z.string(),
          resource: z.literal("subscriptions"),
          items: z.array(podcastSubscriptionSchema),
        }),
        pageSchema.extend({
          account: z.string(),
          resource: z.literal("queue"),
          items: z.array(queuedEpisodeSchema),
        }),
        pageSchema.extend({
          account: z.string(),
          resource: z.literal("inbox"),
          items: z.array(inboxEpisodeSchema),
        }),
        pageSchema.extend({
          account: z.string(),
          resource: z.literal("listen_history"),
          items: z.array(listenedEpisodeSchema),
        }),
      ]),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: [
          "Reads the configured podcast account through its rate-limited client",
        ],
        cost: "No expected monetary cost; consumes bounded private account API traffic",
        recommendedPolicy: "allow",
      },
      execute: ({ resource, sinceDays, query, cursor, limit }) =>
        Effect.gen(function* () {
          const account = yield* requirePodcastAccountEffect(runtime);
          if (resource === "subscriptions") {
            const values = matchesQuery(
              requireAvailable(yield* account.fetchSubscriptions()),
              query,
            );
            return {
              account: account.name,
              resource,
              ...paginate(values, cursor, limit),
            };
          }
          if (resource === "queue") {
            const values = matchesQuery(
              requireAvailable(yield* account.fetchQueue()),
              query,
            );
            return {
              account: account.name,
              resource,
              ...paginate(values, cursor, limit),
            };
          }
          if (resource === "inbox") {
            const values = matchesQuery(
              requireAvailable(yield* account.fetchInbox()),
              query,
            );
            return {
              account: account.name,
              resource,
              ...paginate(values, cursor, limit),
            };
          }
          const values = matchesQuery(
            requireAvailable(
              yield* account.fetchListenHistory(
                (yield* Clock.currentTimeMillis) - sinceDays * 86_400_000,
              ),
            ),
            query,
          );
          return {
            account: account.name,
            resource,
            ...paginate(values, cursor, limit),
          };
        }),
    }),
    defineTool({
      name: "podcast_account_search",
      title: "Search Podcast Account",
      description:
        "Search shows or episodes through the configured podcast account client. Results are bounded and do not change subscriptions or queue state.",
      inputSchema: z
        .object({
          resource: z.enum(["shows", "episodes"]),
          query: z.string().trim().min(1).max(200),
          cursor: paginationInputShape.cursor,
          limit: z.number().int().min(1).max(50).default(20),
        })
        .strict(),
      outputSchema: z.discriminatedUnion("resource", [
        pageSchema.extend({
          account: z.string(),
          resource: z.literal("shows"),
          items: z.array(podcastSearchResultSchema),
        }),
        pageSchema.extend({
          account: z.string(),
          resource: z.literal("episodes"),
          items: z.array(episodeSearchResultSchema),
        }),
      ]),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: [
          "Searches the configured podcast account through its rate-limited client",
        ],
        cost: "No expected monetary cost; consumes bounded private account API traffic",
        recommendedPolicy: "allow",
      },
      execute: ({ resource, query, cursor, limit }) =>
        Effect.gen(function* () {
          const account = yield* requirePodcastAccountEffect(runtime);
          if (resource === "shows") {
            return {
              account: account.name,
              resource,
              ...paginate(
                requireAvailable(yield* account.searchPodcasts(query)),
                cursor,
                limit,
              ),
            };
          }
          return {
            account: account.name,
            resource,
            ...paginate(
              requireAvailable(yield* account.searchEpisodes(query)),
              cursor,
              limit,
            ),
          };
        }),
    }),
    defineTool({
      name: "podcast_account_update",
      title: "Update Podcast Account",
      description:
        "Enqueue or dequeue an episode, clear an Inbox item, or subscribe to a show. These change an external podcast account and require Executor approval.",
      inputSchema: z.discriminatedUnion("action", [
        z.object({
          action: z.literal("enqueue"),
          feedUrl: z.string().url(),
          itunesId: z.number().int().positive().optional(),
          episodeGuid: z.string().min(1).max(1000),
          mediaUrl: z.string().url().optional(),
          showTitle: z.string().trim().min(1).max(300),
          episodeTitle: z.string().trim().min(1).max(500),
          position: z
            .enum([PodcastQueuePosition.Next, PodcastQueuePosition.Last])
            .default(PodcastQueuePosition.Next),
        }),
        z.object({
          action: z.literal("dequeue"),
          episodeGuid: z.string().min(1).max(1000),
        }),
        z.object({
          action: z.literal("clear_inbox"),
          clientEpisodeId: z.string().min(1).max(500),
        }),
        z.object({
          action: z.literal("subscribe"),
          title: z.string().trim().min(1).max(300),
          feedUrl: z.string().url(),
          itunesId: z.number().int().positive().optional(),
        }),
      ]),
      outputSchema: z.object({
        account: z.string(),
        action: z.enum(["enqueue", "dequeue", "clear_inbox", "subscribe"]),
        result: z.enum([
          "added",
          "removed",
          "already_exists",
          "not_found",
          "unavailable",
          "error",
        ]),
      }),
      annotations: annotations(false, true, true, true),
      policy: {
        sideEffects: [
          "Changes queue, Inbox, or subscription state on an external podcast account",
        ],
        cost: "No expected monetary cost; consumes private account API traffic",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const account = yield* requirePodcastAccountEffect(runtime);
          let result: PodcastWriteResult;
          if (input.action === "enqueue") {
            const request: EnqueueEpisodeRequest = {
              feedUrl: input.feedUrl,
              itunesId: input.itunesId,
              episodeGuid: input.episodeGuid,
              mediaUrl: input.mediaUrl,
              showTitle: input.showTitle,
              episodeTitle: input.episodeTitle,
              position: input.position,
            };
            result = yield* account.enqueueEpisode(request);
          } else if (input.action === "dequeue") {
            result = yield* account.dequeueEpisode(input.episodeGuid);
          } else if (input.action === "clear_inbox") {
            result = yield* account.clearInboxEpisode(input.clientEpisodeId);
          } else {
            result = yield* account.subscribeToShow(input);
          }
          return { account: account.name, action: input.action, result };
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "podcast_recommendations_list",
      title: "List Podcast Recommendations",
      description: "List persisted podcast episode recommendations and outcomes.",
      inputSchema: z
        .object({
          status: z
            .enum(["pending", "notified", "listened", "abandoned", "ignored", "failed"])
            .optional(),
          feedback: z.enum(["good_pick", "not_for_me", "none"]).optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: pageSchema.extend({ items: z.array(podcastRecommendationSchema) }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ status, feedback, cursor, limit }) =>
        Effect.sync(() => {
          let values = getAllPodcastRecommendations();
          if (status) values = values.filter((item) => item.status === status);
          if (feedback) {
            values = values.filter((item) =>
              feedback === "none" ? !item.feedback : item.feedback === feedback,
            );
          }
          return paginate(values.map(serializePodcastRecommendation), cursor, limit);
        }),
    }),
    defineTool({
      name: "podcast_recommendation_get",
      title: "Get Podcast Recommendation",
      description: "Get one persisted podcast recommendation by recommendation ID.",
      inputSchema: z.object({ recommendationId: z.string().min(1).max(300) }).strict(),
      outputSchema: z.object({ recommendation: podcastRecommendationSchema }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ recommendationId }) =>
        Effect.sync(() => {
          const value = getPodcastRecommendation(recommendationId);
          if (!value) throw new Error("Podcast recommendation not found");
          return { recommendation: serializePodcastRecommendation(value) };
        }),
    }),
    defineTool({
      name: "podcast_recommendation_feedback",
      title: "Record Podcast Recommendation Feedback",
      description:
        "Record good-pick or not-for-me feedback and/or a bounded note. This changes only Omni's local recommendation state.",
      inputSchema: z
        .object({
          recommendationId: z.string().min(1).max(300),
          feedback: z.enum(["good_pick", "not_for_me"]).optional(),
          note: z.string().trim().max(1000).optional(),
        })
        .strict()
        .refine((value) => value.feedback !== undefined || value.note !== undefined, {
          message: "feedback or note is required",
        }),
      outputSchema: z.object({ recommendation: podcastRecommendationSchema }),
      annotations: annotations(false, false, false, false),
      policy: {
        sideEffects: ["Updates local podcast feedback used by future taste analysis"],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ recommendationId, feedback, note }) =>
        Effect.sync(() => {
          const value = setPodcastRecommendationFeedback(recommendationId, {
            feedback,
            note,
          });
          if (!value) throw new Error("Podcast recommendation not found");
          return { recommendation: serializePodcastRecommendation(value) };
        }),
    }),
    defineTool({
      name: "podcast_taste_read",
      title: "Read Podcast Taste Data",
      description:
        "Read the latest derived podcast taste profile or paginated evidence supporting it.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({ resource: z.literal("profile") }),
        z.object({
          resource: z.literal("evidence"),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        }),
      ]),
      outputSchema: z.discriminatedUnion("resource", [
        z.object({ resource: z.literal("profile"), profile: profileSchema.nullable() }),
        pageSchema.extend({
          resource: z.literal("evidence"),
          items: z.array(
            z.object({
              evidenceId: z.string(),
              kind: z.enum(["listen", "recommendation_outcome", "explicit_feedback"]),
              showKey: z.string(),
              showTitle: z.string(),
              episodeTitle: nullableString,
              observedAt: z.number().int(),
              completion: nullableNumber,
              recommendationId: nullableString,
              feedback: z.enum(["good_pick", "not_for_me"]).nullable(),
              note: nullableString,
            }),
          ),
        }),
      ]),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.sync(() => {
          if (input.resource === "profile") {
            return {
              resource: "profile",
              profile: getLatestPodcastTasteProfile() ?? null,
            };
          }
          const values = getAllPodcastTasteEvidence().map((item) => ({
            evidenceId: item.evidenceId,
            kind: item.kind,
            showKey: item.showKey,
            showTitle: item.showTitle,
            episodeTitle: item.episodeTitle ?? null,
            observedAt: item.observedAt,
            completion: item.completion ?? null,
            recommendationId: item.recommendationId ?? null,
            feedback: item.feedback ?? null,
            note: item.note ?? null,
          }));
          return {
            resource: "evidence",
            ...paginate(values, input.cursor, input.limit),
          };
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "presspods_list",
      title: "List PressPods Resources",
      description:
        "List persisted PressPods episodes or queued, processing, and failed jobs. Audio filenames and bytes are not exposed.",
      inputSchema: z
        .object({
          resource: z.enum(["episodes", "jobs"]),
          status: z.enum(["queued", "processing", "failed"]).optional(),
          query: z.string().trim().max(300).optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: z.discriminatedUnion("resource", [
        pageSchema.extend({
          resource: z.literal("episodes"),
          items: z.array(pressPodsEpisodeSchema),
        }),
        pageSchema.extend({
          resource: z.literal("jobs"),
          items: z.array(pressPodsJobSchema),
        }),
      ]),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ resource, status, query, cursor, limit }) =>
        Effect.gen(function* () {
          if (resource === "episodes") {
            let values = yield* PressPodsPersistence.getAllEpisodes();
            if (query) {
              const needle = query.toLocaleLowerCase();
              values = values.filter((item) =>
                [item.title, item.author, item.publication, item.domain].some((value) =>
                  value?.toLocaleLowerCase().includes(needle),
                ),
              );
            }
            return {
              resource,
              ...paginate(values.map(serializeEpisode), cursor, limit),
            };
          }
          let values = yield* PressPodsPersistence.getAllJobs();
          if (status) values = values.filter((item) => item.status === status);
          if (query) {
            const needle = query.toLocaleLowerCase();
            values = values.filter((item) =>
              item.url.toLocaleLowerCase().includes(needle),
            );
          }
          return { resource, ...paginate(values.map(serializeJob), cursor, limit) };
        }),
    }),
    defineTool({
      name: "presspods_episode_get",
      title: "Get PressPods Episode",
      description:
        "Get compact metadata for one PressPods episode. Use presspods_transcript_read for bounded narration text.",
      inputSchema: z.object({ episodeId: z.string().min(1).max(200) }).strict(),
      outputSchema: z.object({ episode: pressPodsEpisodeSchema }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ episodeId }) =>
        Effect.gen(function* () {
          const episode = yield* PressPodsPersistence.getEpisode(episodeId);
          if (!episode)
            return yield* Effect.fail(new Error("PressPods episode not found"));
          return { episode: serializeEpisode(episode) };
        }),
    }),
    defineTool({
      name: "presspods_transcript_read",
      title: "Read PressPods Transcript",
      description:
        "Read a bounded page of an episode's cleaned narration transcript. This never returns audio bytes or filesystem paths.",
      inputSchema: z
        .object({
          episodeId: z.string().min(1).max(200),
          offset: z.number().int().min(0).default(0),
          maxChars: z.number().int().min(1).max(10_000).default(4000),
        })
        .strict(),
      outputSchema: z.object({
        episodeId: z.string(),
        title: z.string(),
        offset: z.number().int().nonnegative(),
        text: z.string(),
        nextOffset: z.number().int().nonnegative().nullable(),
        totalChars: z.number().int().nonnegative(),
        truncated: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ episodeId, offset, maxChars }) =>
        Effect.gen(function* () {
          const episode = yield* PressPodsPersistence.getEpisode(episodeId);
          if (!episode)
            return yield* Effect.fail(new Error("PressPods episode not found"));
          if (offset > episode.content.length)
            return yield* Effect.fail(
              new Error("Transcript offset is beyond the end of the episode"),
            );
          const page = truncate(episode.content.slice(offset), maxChars);
          return {
            episodeId,
            title: episode.title,
            offset,
            text: page.text,
            nextOffset: page.truncated ? offset + page.text.length : null,
            totalChars: episode.content.length,
            truncated: page.truncated,
          };
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "presspods_submit",
      title: "Submit PressPods Episode",
      description:
        "Queue a public article URL for retrieval, model cleaning, TTS synthesis, podcast publication, optional Karakeep bookmarking, and notification. This has material compute/model cost and external effects, so Executor approval is required.",
      inputSchema: submitEpisodeSchema.strict(),
      outputSchema: z.object({ job: pressPodsJobSchema }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Queues paid or self-hosted model and TTS work",
          "May bookmark the URL in Karakeep",
          "Publishes an episode to the personal feed and sends a notification after processing",
        ],
        cost: "May incur metadata/cleaning model and TTS charges; ElevenLabs is approximately $0.10 per 1,000 characters when configured",
        recommendedPolicy: "require_approval",
      },
      execute: ({ url }) =>
        Effect.gen(function* () {
          yield* assertPublicHttpUrl(url);
          const job = yield* submitEpisodeUrlEffect(
            url,
            () => kickPressPods(runtime),
            runtime.logger.extend("MCP:PressPods"),
          );
          return { job: serializeJob(job) };
        }),
    }),
    defineTool({
      name: "presspods_retry",
      title: "Retry PressPods Work",
      description:
        "Regenerate an existing episode or retry a failed job. This starts model/TTS work and can replace a published episode, so Executor approval is required.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("episode"),
          episodeId: z.string().min(1).max(200),
        }),
        z.object({ resource: z.literal("job"), jobId: z.string().min(1).max(200) }),
      ]),
      outputSchema: z.object({ job: pressPodsJobSchema }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Queues paid or self-hosted model and TTS work",
          "May replace an existing published episode and send a notification",
        ],
        cost: "May incur metadata/cleaning model and TTS charges",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          let job: PressPodsJobData;
          if (input.resource === "episode") {
            const episode = yield* PressPodsPersistence.getEpisode(input.episodeId);
            if (!episode)
              return yield* Effect.fail(new Error("PressPods episode not found"));
            job = yield* submitEpisodeUrlEffect(
              episode.articleUrl,
              () => kickPressPods(runtime),
              runtime.logger.extend("MCP:PressPods"),
            );
          } else {
            const existing = yield* PressPodsPersistence.getJob(input.jobId);
            if (!existing)
              return yield* Effect.fail(new Error("PressPods job not found"));
            if (existing.status !== "failed")
              return yield* Effect.fail(
                new Error("Only failed PressPods jobs can be retried"),
              );
            const requeued = yield* PressPodsPersistence.requeueJobNow(input.jobId);
            if (!requeued)
              return yield* Effect.fail(
                new Error("PressPods job could not be retried"),
              );
            job = requeued;
            kickPressPods(runtime);
          }
          return { job: serializeJob(job) };
        }),
    }),
    defineTool({
      name: "presspods_delete",
      title: "Delete PressPods Resource",
      description:
        "Permanently delete an episode and its audio, or dismiss a non-processing job and its resume checkpoints. This is destructive and requires Executor approval.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("episode"),
          episodeId: z.string().min(1).max(200),
        }),
        z.object({ resource: z.literal("job"), jobId: z.string().min(1).max(200) }),
      ]),
      outputSchema: z.object({
        resource: z.enum(["episode", "job"]),
        deleted: z.literal(true),
      }),
      annotations: annotations(false, true, true, false),
      policy: {
        sideEffects: [
          "Permanently removes local episode/audio data or a queued/failed job and its checkpoints",
        ],
        cost: "No monetary cost",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          if (input.resource === "episode") {
            const episode = yield* PressPodsPersistence.deleteEpisode(input.episodeId);
            if (!episode)
              return yield* Effect.fail(new Error("PressPods episode not found"));
            yield* deleteEpisodeAudio(episode.audioFile);
          } else {
            const job = yield* PressPodsPersistence.getJob(input.jobId);
            if (!job) return yield* Effect.fail(new Error("PressPods job not found"));
            if (job.status === "processing")
              return yield* Effect.fail(
                new Error("A processing PressPods job cannot be dismissed"),
              );
            yield* PressPodsPersistence.deleteJob(job.jobId);
            yield* clearChunkCheckpoints(checkpointWorkId(jobNormalizedUrl(job)));
          }
          return { resource: input.resource, deleted: true };
        }),
    }),
  );

  tools.push(
    defineTool({
      name: "pets_read",
      title: "Read Pet Weight Data",
      description:
        "List pets with bounded recent weight and visit history, or read a bounded slice for one pet. This uses only Omni's local synchronized data.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("list"),
          historyLimit: z.number().int().min(0).max(100).default(10),
        }),
        z.object({
          resource: z.literal("history"),
          petId: z.string().min(1).max(200),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        }),
      ]),
      outputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("list"),
          pets: z.array(
            z.object({
              petId: z.string(),
              name: z.string(),
              currentWeight: z.number(),
              updatedAt: z.string(),
              recentWeights: z.array(
                z.object({ timestamp: z.string(), weight: z.number() }),
              ),
              recentVisits: z.array(
                z.object({ date: z.string(), count: z.number().int().nonnegative() }),
              ),
            }),
          ),
        }),
        pageSchema.extend({
          resource: z.literal("history"),
          pet: z.object({
            petId: z.string(),
            name: z.string(),
            currentWeight: z.number(),
            updatedAt: z.string(),
          }),
          items: z.array(z.object({ timestamp: z.string(), weight: z.number() })),
        }),
      ]),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.sync(() => {
          if (input.resource === "list") {
            return {
              resource: "list",
              pets: getAllPetsWithHistory().map((pet) => ({
                petId: pet.pet_id,
                name: pet.name,
                currentWeight: pet.current_weight,
                updatedAt: pet.updated_at,
                recentWeights: pet.weightHistory
                  .slice(-input.historyLimit)
                  .map((row) => ({ timestamp: row.timestamp, weight: row.weight })),
                recentVisits: getDailyVisitCounts(pet.pet_id).slice(
                  -input.historyLimit,
                ),
              })),
            };
          }
          const pet = getPet(input.petId);
          if (!pet) throw new Error("Pet not found");
          return {
            resource: "history",
            pet: {
              petId: pet.pet_id,
              name: pet.name,
              currentWeight: pet.current_weight,
              updatedAt: pet.updated_at,
            },
            ...paginate(
              getWeightHistory(input.petId).map((row) => ({
                timestamp: row.timestamp,
                weight: row.weight,
              })),
              input.cursor,
              input.limit,
            ),
          };
        }),
    }),
    defineTool({
      name: "costs_read",
      title: "Read Omni Cost Telemetry",
      description:
        "Summarize Omni's persisted model, search, TTS, retrieval, and transcription costs for a fixed time range. Unknown-price events remain explicit.",
      inputSchema: z
        .object({
          days: z.union([z.literal(7), z.literal(30), z.literal(90)]).default(30),
        })
        .strict(),
      outputSchema: z.object({
        range: z.object({
          days: z.number().nullable(),
          from: nullableNumber,
          to: z.number(),
        }),
        summary: z.object({
          selectedCostCents: z.number(),
          allTimeCostCents: z.number(),
          allTimeUnknownEventCount: z.number().int().nonnegative(),
          averageDailyCostCents: z.number(),
          highestDay: z.object({ date: z.string(), costCents: z.number() }).nullable(),
          eventCount: z.number().int().nonnegative(),
          unknownEventCount: z.number().int().nonnegative(),
          inputTokens: z.number().nonnegative(),
          outputTokens: z.number().nonnegative(),
          characters: z.number().nonnegative(),
          requests: z.number().nonnegative(),
          credits: z.number().nonnegative(),
        }),
        daily: z.array(
          z.object({
            date: z.string(),
            costCents: z.number(),
            byFeature: z.record(z.string(), z.number()),
            pricedEventCount: z.number().int().nonnegative(),
            unknownEventCount: z.number().int().nonnegative(),
          }),
        ),
        byFeature: z.array(
          z.object({
            feature: z.string(),
            costCents: z.number(),
            eventCount: z.number().int().nonnegative(),
            unknownEventCount: z.number().int().nonnegative(),
          }),
        ),
        byService: z.array(
          z.object({
            service: z.string(),
            model: nullableString,
            category: z.string(),
            costCents: z.number(),
            eventCount: z.number().int().nonnegative(),
            unknownEventCount: z.number().int().nonnegative(),
            inputTokens: z.number().nonnegative(),
            inputNoCacheTokens: z.number().nonnegative(),
            cacheReadTokens: z.number().nonnegative(),
            cacheWriteTokens: z.number().nonnegative(),
            outputTokens: z.number().nonnegative(),
            reasoningTokens: z.number().nonnegative(),
            characters: z.number().nonnegative(),
            requests: z.number().nonnegative(),
            credits: z.number().nonnegative(),
          }),
        ),
        recent: z.array(
          z.object({
            eventId: z.string(),
            incurredAt: z.number(),
            category: z.string(),
            feature: z.string(),
            operation: z.string(),
            service: z.string(),
            model: nullableString,
            costCents: nullableNumber,
            priceStatus: z.string(),
            usage: costUsageSchema,
            runId: nullableString,
          }),
        ),
      }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ days }) =>
        Effect.sync(() => {
          const count = CostEventEntity.count();
          if (count > MAX_MCP_COST_EVENTS) {
            throw new Error(
              `Cost telemetry exceeds the MCP scan limit (${count} events; maximum ${MAX_MCP_COST_EVENTS})`,
            );
          }
          return summarizeCosts(getCostEvents(), { days, timeZone: config.TZ });
        }),
    }),
  );

  return tools;
}
