import { Clock, Effect } from "effect";
import { z } from "zod";
import {
  type EnqueueEpisodeRequest,
  PodcastQueuePosition,
  type PodcastWriteResult,
  resolvePodcastAccountEffect,
} from "../../podcast-recs/account.js";
import {
  getAllPodcastRecommendations,
  getPodcastRecommendation,
  type PodcastRecommendationData,
  setPodcastRecommendationFeedback,
} from "../../podcast-recs/persistence.js";
import {
  getAllPodcastTasteEvidence,
  getLatestPodcastTasteProfile,
} from "../../podcast-recs/reflection/persistence.js";
import type { McpRuntime } from "../runtime.js";
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

function serializePodcastRecommendation(rec: PodcastRecommendationData) {
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

export function createPodcastTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
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
        Effect.gen(function* () {
          let values = yield* getAllPodcastRecommendations();
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
        Effect.gen(function* () {
          const value = yield* getPodcastRecommendation(recommendationId);
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
        Effect.gen(function* () {
          const value = yield* setPodcastRecommendationFeedback(recommendationId, {
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
        Effect.gen(function* () {
          if (input.resource === "profile") {
            return {
              resource: "profile",
              profile: getLatestPodcastTasteProfile() ?? null,
            };
          }
          const values = (yield* getAllPodcastTasteEvidence()).map((item) => ({
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
  ];
}
