import { Effect } from "effect";
import { z } from "zod";
import {
  fetchInProgressEffect,
  fetchLibraryIndexEffect,
  fetchWatchHistoryEffect,
} from "../../recommendations/mediaLibrary.js";
import {
  getAllRecommendations,
  getRecommendation,
  type RecommendationData,
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
  annotations,
  defineTool,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
} from "../tool.js";
import { pageSchema, profileSchema, requireAvailable } from "./media-shared.js";

const nullableString = z.string().nullable();

const nullableNumber = z.number().nullable();

const mediaTypeSchema = z.enum([MediaType.Movie, MediaType.Tv]);

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

function serializeRecommendation(rec: RecommendationData) {
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

export function createMediaTools(): McpToolDefinition[] {
  return [
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
        Effect.gen(function* () {
          let values = yield* getAllRecommendations();
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
        Effect.gen(function* () {
          const value = yield* getRecommendation(recommendationId);
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
        Effect.gen(function* () {
          const value = yield* setRecommendationFeedback(recommendationId, {
            feedback,
            note,
          });
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
        Effect.gen(function* () {
          if (input.resource === "profile") {
            return { resource: "profile", profile: getLatestTasteProfile() ?? null };
          }
          const values = (yield* getAllTasteEvidence()).map((item) => ({
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
  ];
}
