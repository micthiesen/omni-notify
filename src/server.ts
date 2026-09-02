import type { Effect as EffectType } from "effect/Effect";
import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import type { Logger } from "@micthiesen/mitools/logging";
import { type Context, Hono } from "hono";
import { streamSSE } from "hono/streaming";
import {
  Clock,
  Deferred,
  Duration,
  Effect,
  Queue,
  Schedule,
  Schema,
  Stream,
} from "effect";
import { getAllBriefingHistories } from "./briefing-agent/persistence.js";
import {
  AUTO_PASS_SENDERS as CALENDAR_BUILTIN_AUTO_PASS,
  BLACKLISTED_SENDERS as CALENDAR_BUILTIN_BLOCKED,
} from "./calendar-events/filter/keywords.js";
import { getCostEvents } from "./costs/persistence.js";
import { summarizeCosts } from "./costs/summary.js";
import {
  deleteManagedEntityRow,
  getManagedDataSummary,
  getManagedEntity,
  listManagedEntities,
} from "./data-manager.js";
import {
  type EmailPipelineName,
  getEmailActivity,
  getRecentEmailActivity,
  KEEP_PER_PIPELINE,
} from "./email/activity.js";
import { getEmailActivityLogs } from "./email/activityLogs.js";
import {
  deleteEmailFeedback,
  type EmailFeedbackVerdict,
  listEmailFeedback,
  recordEmailFeedback,
} from "./email/feedback.js";
import { clearEmailRetry } from "./email/retry.js";
import {
  deleteEmailRule,
  type EmailRuleScope,
  type EmailRuleVerdict,
  listEmailRules,
  normalizeRulePattern,
  upsertEmailRuleChecked,
} from "./email/senderRules.js";
import type { EmailHandler, EmailTransport } from "./email/types.js";
import { registerIOSControlRoutes } from "./ios-controls/routes.js";
import type { IOSControlService } from "./ios-controls/service.js";
import {
  type LiveDisplayItem,
  sortLiveDisplay,
  sortOfflineDisplay,
} from "./live-check/displayOrder.js";
import {
  getLivestreamDiagnostics,
  getLivestreamEvents,
  getLivestreamIntelligence,
  recordLivestreamFeedback,
} from "./live-check/intelligence/persistence.js";
import type { LivestreamIntelligenceDiagnosticsProvider } from "./live-check/intelligence/service.js";
import {
  getPlatformViewerMetrics,
  getViewerMetricsEffect,
} from "./live-check/metrics/persistence.js";
import { getStreamerStatusEffect } from "./live-check/persistence.js";
import { platformConfigs } from "./live-check/platforms/index.js";
import { getStreamSessions } from "./live-check/sessions.js";
import {
  type PlatformBinding,
  type Streamer,
  streamerOrderingViewerCount,
} from "./live-check/streamers.js";
import { toTriggerChannels } from "./live-check/triggerChannels.js";
import { registerOmniMcpRoute } from "./mcp/route.js";
import {
  effectHandler,
  effectMiddleware,
  HttpBodyTooLargeError,
  readJsonBody,
} from "./effect/http.js";
import { IntegrationError } from "./effect/errors.js";
import { fromPromise, fromSync, runPromise } from "./effect/interop.js";
import { awaitSseWriter } from "./effect/sse.js";
import {
  CARRIER_SENDER_DOMAINS as PARCEL_BUILTIN_AUTO_PASS,
  BLACKLISTED_SENDERS as PARCEL_BUILTIN_BLOCKED,
} from "./parcel-tracker/filter/keywords.js";
import { SubmittedDeliveryEntity } from "./parcel-tracker/persistence.js";
import {
  getAllPetsWithHistory,
  getDailyVisitCounts,
  getPet,
  getWeightHistory,
} from "./pet-tracker/persistence.js";
import {
  getAllPodcastRecommendations,
  getPodcastRecommendation,
  type PodcastRecommendationData,
  setPodcastRecommendationFeedback,
} from "./podcast-recs/persistence.js";
import { MAX_PODCAST_RECOMMENDATIONS_PER_RUN } from "./podcast-recs/pipeline.js";
import { getLatestPodcastTasteProfile } from "./podcast-recs/reflection/index.js";
import { registerPressPodsRoutes } from "./press-pods/routes.js";
import { createPrinterService } from "./printer/service.js";
import {
  getAllRecommendations,
  getOpenRecommendations,
  getRecommendation,
  type RecommendationData,
  selectOnDeck,
  setRecommendationFeedback,
} from "./recommendations/persistence.js";
import { MAX_RECOMMENDATIONS_PER_RUN } from "./recommendations/pipeline.js";
import { getLatestTasteProfile } from "./recommendations/taste/index.js";
import { runLogBus, taskRunBus } from "./task-runs/events.js";
import { getActiveRunLogs } from "./task-runs/logCapture.js";
import {
  getRun,
  getRunLogs,
  getRuns,
  type TaskRunData,
} from "./task-runs/persistence.js";
import {
  TaskAlreadyRunningError,
  TaskManualInputUnsupportedError,
  TaskNotFoundError,
  type TaskRegistry,
} from "./task-runs/registry.js";
import config from "./utils/config.js";
import {
  approveWorkspaceActionEffect,
  rejectWorkspaceActionEffect,
} from "./workspaces/actions.js";
import {
  getWorkspaceDefinition,
  workspaceDefinitions,
} from "./workspaces/definitions.js";
import {
  getLatestWorkspaceArtifacts,
  getWorkspaceEmailScope,
  getWorkspaceSubject,
  listWorkspaceActions,
  listWorkspaceArtifactRevisions,
  listWorkspaceMessages,
  listWorkspacePapercuts,
  listWorkspaceSources,
  listWorkspaceSubjects,
  resolveWorkspacePapercut,
  upsertWorkspaceSubject,
} from "./workspaces/persistence.js";

function round(n: number): number {
  return Math.round(n * 100) / 100;
}

// Optional fields are normalized to explicit nulls so the wire format matches
// the frontend's types (JSON.stringify would silently drop undefined keys).
function serializeRun(run: TaskRunData) {
  return {
    runId: run.runId,
    taskName: run.taskName,
    trigger: run.trigger,
    scheduledFor: run.scheduledFor ?? null,
    startedAt: run.startedAt,
    finishedAt: run.finishedAt ?? null,
    status: run.status,
    error: run.error ?? null,
    summary: run.summary ?? null,
  };
}

function serializeWorkspaceEmailScope(
  scope: ReturnType<typeof getWorkspaceEmailScope>,
) {
  if (!scope) return null;
  return {
    senders: scope.senders,
    domains: scope.domains,
    subjectKeywords: scope.subjectKeywords,
    bodyKeywords: scope.bodyKeywords,
  };
}

// Radarr's movie page slug is the TMDB id, so link straight to it once the movie
// is in the library; otherwise land on the add-new search pre-filled by tmdb id.
// Sonarr's detail pages need its own titleSlug, captured at add time as
// managerSlug; rows from before that was recorded fall back to the add-new
// search, which shows an existing series as such and links through to it.
function buildManagerLink(rec: RecommendationData): string {
  if (rec.mediaType === "movie") {
    const inRadarr =
      rec.watchlistResult === "added" || rec.watchlistResult === "already_exists";
    return inRadarr
      ? `http://radarr.boris/movie/${rec.tmdbId}`
      : `http://radarr.boris/add/new?term=${encodeURIComponent(`tmdb:${rec.tmdbId}`)}`;
  }
  return rec.managerSlug
    ? `http://sonarr.boris/series/${rec.managerSlug}`
    : `http://sonarr.boris/add/new?term=${encodeURIComponent(rec.title)}`;
}

function serializeRecommendation(rec: RecommendationData) {
  return {
    recommendationId: rec.recommendationId,
    canonicalId: rec.canonicalId,
    tmdbId: rec.tmdbId,
    mediaType: rec.mediaType,
    title: rec.title,
    year: rec.year ?? null,
    posterPath: rec.posterPath ?? null,
    status: rec.status,
    whyForUser: rec.whyForUser ?? null,
    caveats: rec.caveats ?? [],
    runDate: rec.runDate,
    recommendedAt: rec.recommendedAt,
    notifiedAt: rec.notifiedAt ?? null,
    startedAt: rec.startedAt ?? null,
    resolvedAt: rec.resolvedAt ?? null,
    watchlistResult: rec.watchlistResult ?? null,
    confidence: rec.confidence ?? null,
    feedback: rec.feedback ?? null,
    feedbackAt: rec.feedbackAt ?? null,
    feedbackNote: rec.feedbackNote ?? null,
    source: rec.source ?? null,
    genres: rec.genres ?? [],
    runtimeMinutes: rec.runtimeMinutes ?? null,
    seasonCount: rec.seasonCount ?? null,
    episodeCount: rec.episodeCount ?? null,
    seriesStatus: rec.seriesStatus ?? null,
    originalLanguage: rec.originalLanguage ?? null,
    originCountries: rec.originCountries ?? [],
    creators: rec.creators ?? [],
    cast: rec.cast ?? [],
    keywords: rec.keywords ?? [],
    certification: rec.certification ?? null,
    shortlistScores: rec.shortlistScores ?? null,
    links: {
      tmdb: `https://www.themoviedb.org/${rec.mediaType}/${rec.tmdbId}`,
      plex: `http://plex.boris/web/index.html#!/search?pivot=top&query=${encodeURIComponent(rec.title)}`,
      manager: buildManagerLink(rec),
    },
  };
}

/**
 * Compact "On Deck" strip for the dashboard: the newest delivered media
 * recommendations still awaiting an outcome (see selectOnDeck for the pure
 * filter/sort/cap logic).
 */
function buildOnDeck() {
  return selectOnDeck(getOpenRecommendations()).map((rec) => ({
    recommendationId: rec.recommendationId,
    title: rec.title,
    mediaType: rec.mediaType,
    year: rec.year ?? null,
    posterPath: rec.posterPath ?? null,
    whyForUser: rec.whyForUser ?? null,
    recommendedAt: rec.recommendedAt,
  }));
}

function serializePodcastRecommendation(rec: PodcastRecommendationData) {
  return {
    recommendationId: rec.recommendationId,
    showTitle: rec.showTitle,
    episodeTitle: rec.episodeTitle,
    feedUrl: rec.feedUrl,
    itunesId: rec.itunesId ?? null,
    artworkUrl: rec.artworkUrl ?? null,
    episodeUrl: rec.episodeUrl ?? null,
    publishedAt: rec.publishedAt,
    durationMinutes: rec.durationMinutes ?? null,
    status: rec.status,
    whyForUser: rec.whyForUser ?? null,
    caveats: rec.caveats ?? [],
    confidence: rec.confidence ?? null,
    shortlistScores: rec.shortlistScores ?? null,
    discoveredVia: rec.discoveredVia ?? null,
    sourceUrl: rec.sourceUrl ?? null,
    matchedVoices: rec.matchedVoices ?? [],
    recommendedAt: rec.recommendedAt,
    notifiedAt: rec.notifiedAt ?? null,
    queueResult: rec.queueResult ?? null,
    feedback: rec.feedback ?? null,
    feedbackAt: rec.feedbackAt ?? null,
    feedbackNote: rec.feedbackNote ?? null,
  };
}

// Entity data round-trips through JSON, so Date fields come back as ISO
// strings at runtime regardless of their declared type.
function toEpochMs(value: Date | string): number {
  return new Date(value).getTime();
}

function serializeBinding(binding: PlatformBinding) {
  return {
    platform: binding.platform,
    username: binding.username,
    url:
      binding.urlOverride ??
      platformConfigs[binding.platform].getLiveUrl(binding.username),
  };
}

function serializeStreamer(streamer: Streamer) {
  return Effect.gen(function* () {
    const status = yield* getStreamerStatusEffect(streamer.id);
    const intelligence = status.isLive
      ? yield* fromSync("read livestream intelligence", () =>
          getLivestreamIntelligence(streamer.id),
        )
      : undefined;
    const base = {
      id: streamer.id,
      displayName: streamer.displayName,
      bindings: streamer.bindings.map(serializeBinding),
      tier: streamer.tier,
      dgg: streamer.dgg,
    };
    if (status.isLive) {
      return {
        ...base,
        live: true as const,
        title: status.primaryTitle,
        startedAt: toEpochMs(status.startedAt),
        maxViewerCount: status.maxViewerCount,
        // Current (not max) summed viewer count; null when unknown/0-data —
        // e.g. rows persisted before this field existed.
        viewerCount: status.viewerCount ?? null,
        sources: (status.sources ?? []).map((source) => ({
          ...source,
          viewerCount: source.viewerCount ?? null,
        })),
        category: status.category ?? null,
        primary: serializeBinding(status.primary),
        intelligence,
      };
    }
    return {
      ...base,
      live: false as const,
      lastStartedAt: status.lastStartedAt ? toEpochMs(status.lastStartedAt) : null,
      lastEndedAt: status.lastEndedAt ? toEpochMs(status.lastEndedAt) : null,
      lastMaxViewerCount: status.lastMaxViewerCount ?? null,
    };
  });
}

function serializeStreamersForDisplay(streamers: Streamer[]) {
  type SerializedStreamer = Effect.Success<ReturnType<typeof serializeStreamer>>;
  type SerializedLive = Extract<SerializedStreamer, { live: true }>;
  type SerializedOffline = Extract<SerializedStreamer, { live: false }>;

  const live: Array<LiveDisplayItem & { serialized: SerializedLive }> = [];
  const offline: SerializedOffline[] = [];
  return Effect.gen(function* () {
    const serializedStreamers = yield* Effect.forEach(streamers, (streamer) =>
      serializeStreamer(streamer).pipe(
        Effect.map((serialized) => ({ streamer, serialized })),
      ),
    );
    for (const { streamer, serialized } of serializedStreamers) {
      if (serialized.live) {
        live.push({
          serialized,
          tier: serialized.tier,
          viewerCount: serialized.viewerCount,
          maxViewerCount: serialized.maxViewerCount,
          orderingViewerCount: streamerOrderingViewerCount(streamer),
        });
      } else {
        offline.push(serialized);
      }
    }
    return [
      ...sortLiveDisplay(live).map((entry) => entry.serialized),
      ...sortOfflineDisplay(offline),
    ];
  });
}

const SNAPSHOT_RUN_LIMIT = 30;
const SSE_DEBOUNCE_MS = 150;
const SSE_HEARTBEAT_MS = 25_000;

const requestJsonEffect = (c: Context): EffectType<unknown, HttpBodyTooLargeError> =>
  readJsonBody(c).pipe(Effect.catchTag("HttpBodyError", () => Effect.succeed(null)));

const rejectOversizedJson = <A, E, R>(
  c: Context,
  effect: EffectType<A, E | HttpBodyTooLargeError, R>,
) =>
  effect.pipe(
    Effect.catchTag("HttpBodyTooLargeError", () =>
      Effect.succeed<Response>(c.json({ error: "Request body too large" }, 413)),
    ),
  );

/** Maps manual-run registry errors to responses; anything else rethrows. */
function taskRunErrorResponse(c: Context, error: unknown): Response {
  if (error instanceof TaskNotFoundError) {
    return c.json({ error: error.message }, 404);
  }
  if (error instanceof TaskAlreadyRunningError) {
    return c.json({ error: error.message }, 409);
  }
  if (error instanceof TaskManualInputUnsupportedError) {
    return c.json({ error: error.message }, 400);
  }
  throw error;
}

// Both manual recommendation-run endpoints take the same body, differing
// only in the per-run cap.
const runRequestSchema = (max: number) =>
  Schema.Struct({
    maxRecommendations: Schema.Number.check(
      Schema.isInt(),
      Schema.isBetween({ minimum: 1, maximum: max }),
    ),
  });
const runRequestError = (max: number) =>
  `maxRecommendations must be an integer from 1 to ${max}`;

/**
 * Both feedback endpoints share one flow: validate body → look up → reject
 * undelivered rows → persist → return the serialized recommendation.
 */
function feedbackRoute<
  TData extends { status: string },
  TFeedback extends string,
>(options: {
  schema: Schema.Decoder<{
    readonly feedback?: TFeedback;
    readonly note?: string;
  }>;
  get: (id: string) => TData | undefined;
  setFeedback: (
    id: string,
    input: { feedback?: TFeedback; note?: string },
  ) => TData | undefined;
  serialize: (data: TData) => unknown;
}) {
  return effectHandler((c: Context) =>
    rejectOversizedJson(
      c,
      requestJsonEffect(c).pipe(
        Effect.map((body): Response => {
          const parsed = Schema.decodeUnknownExit(options.schema)(body);
          if (parsed._tag === "Failure") {
            return c.json({ error: "Invalid recommendation feedback" }, 400);
          }
          // A rating, a free-form note, or both, but at least one must be present.
          if (parsed.value.feedback === undefined && !parsed.value.note?.trim()) {
            return c.json({ error: "A rating or a note is required" }, 400);
          }
          const id = c.req.param("id") ?? "";
          const existing = options.get(id);
          if (!existing) {
            return c.json({ error: "Recommendation not found" }, 404);
          }
          if (existing.status === "pending" || existing.status === "failed") {
            return c.json(
              { error: "Undelivered recommendations cannot be rated" },
              409,
            );
          }
          const recommendation = options.setFeedback(id, {
            feedback: parsed.value.feedback,
            note: parsed.value.note?.trim() || undefined,
          });
          if (!recommendation) {
            return c.json({ error: "Recommendation not found" }, 404);
          }
          return c.json({ recommendation: options.serialize(recommendation) });
        }),
      ),
    ),
  );
}

function serializeEmailActivity(a: {
  activityId: string;
  pipeline: string;
  emailId: string;
  subject: string;
  from: string;
  receivedAt: number;
  processedAt: number;
  outcome: string;
  detail?: string;
  admitReason?: string;
  admitTier?: string;
  costCents?: number | null;
  items?: string[];
}) {
  return {
    activityId: a.activityId,
    pipeline: a.pipeline,
    emailId: a.emailId,
    subject: a.subject,
    from: a.from,
    receivedAt: a.receivedAt,
    processedAt: a.processedAt,
    outcome: a.outcome,
    detail: a.detail ?? null,
    admitReason: a.admitReason ?? null,
    // Which tier admitted the email (rule/builtin/triage/keyword-fallback/
    // carrier-name) and the LLM cost incurred deciding/extracting it. costCents
    // is null when no priced LLM call was attributable to the row.
    admitTier: a.admitTier ?? null,
    costCents: a.costCents ?? null,
    items: a.items ?? [],
  };
}

/**
 * Representative sender addresses a user rule for `pattern` targets — a domain
 * rule ("@host") also targets subdomains, so include a subdomain probe. Used to
 * test built-in coverage with the same substring semantics the runtime uses.
 */
function ruleSampleSenders(pattern: string): string[] {
  const domain = pattern.startsWith("@")
    ? pattern.slice(1)
    : pattern.includes("@")
      ? null
      : pattern;
  if (domain === null) return [pattern]; // full "local@host" address rule
  return [`probe@${domain}`, `probe@sub.${domain}`];
}

/**
 * True when a user *block* rule for `pattern`/`scope` would be redundant because
 * a built-in blacklist already covers EVERY sender the rule targets (including a
 * subdomain probe for domain rules). Requiring all samples to be covered avoids
 * falsely claiming coverage when a built-in only matches the bare domain but the
 * user rule would also block subdomains.
 */
function matchesBuiltinBlock(pattern: string, scope: EmailRuleScope): boolean {
  const samples = ruleSampleSenders(pattern);
  const coveredBy = (list: string[]) =>
    samples.every((s) => list.some((e) => s.includes(e.toLowerCase())));
  const inParcel = coveredBy(PARCEL_BUILTIN_BLOCKED);
  const inCalendar = coveredBy(CALENDAR_BUILTIN_BLOCKED);
  if (scope === "parcel") return inParcel;
  if (scope === "calendar") return inCalendar;
  return inParcel && inCalendar;
}

/**
 * Live email-pipeline handles for interactive endpoints (reprocess). Filled
 * in by index.ts after the email features start; empty in server-only mode.
 */
export interface EmailControls {
  transport?: EmailTransport;
  handlers?: Map<string, EmailHandler>;
}

export function startServer(
  port: number,
  parentLogger: Logger,
  registry: TaskRegistry,
  streamers: Streamer[],
  emailControls: EmailControls = {},
  iosControls?: IOSControlService,
  livestreamDiagnostics?: LivestreamIntelligenceDiagnosticsProvider,
): EffectType<void, IntegrationError> {
  const logger = parentLogger.extend("Server");
  const app = new Hono();
  const mcp = registerOmniMcpRoute(
    app,
    {
      logger: parentLogger.extend("MCP"),
      registry,
      streamers,
      emailControls,
      iosControls,
      livestreamDiagnostics,
      printer: config.PRINTER_IPP_URL
        ? createPrinterService(config.PRINTER_IPP_URL)
        : undefined,
    },
    config.OMNI_MCP_TOKEN,
  );

  app.use(
    "/api/*",
    effectMiddleware((c, next) =>
      Effect.gen(function* () {
        if (c.req.method !== "GET" && c.req.method !== "HEAD") {
          const origin = c.req.header("Origin");
          const host = c.req.header("Host");
          let sameOrigin = true;
          if (origin) {
            sameOrigin = yield* Effect.try({
              try: () => Boolean(host) && new URL(origin).host === host,
              catch: () => false,
            }).pipe(Effect.catch(() => Effect.succeed(false)));
          }
          if (!sameOrigin) {
            return c.json({ error: "Cross-origin mutations are not allowed" }, 403);
          }
        }
        yield* next;
        c.header("X-Content-Type-Options", "nosniff");
      }),
    ),
  );

  if (iosControls) {
    registerIOSControlRoutes(
      app,
      iosControls,
      config.IOS_CONTROL_AUTH_TOKEN,
      parentLogger,
    );
  }

  const buildSnapshot = () =>
    Effect.gen(function* () {
      const serializedStreamers = yield* serializeStreamersForDisplay(streamers);
      return yield* fromSync("build dashboard snapshot", () => ({
        tasks: registry.list().map((task) => ({
          ...task,
          lastRun: task.lastRun ? serializeRun(task.lastRun) : null,
        })),
        streamers: serializedStreamers,
        runs: getRuns(undefined, SNAPSHOT_RUN_LIMIT).map(serializeRun),
        onDeck: buildOnDeck(),
      }));
    });

  app.get("/api/tasks", (c) =>
    c.json({
      tasks: registry.list().map((task) => ({
        ...task,
        lastRun: task.lastRun ? serializeRun(task.lastRun) : null,
      })),
    }),
  );

  app.get(
    "/api/streamers",
    effectHandler((c) =>
      serializeStreamersForDisplay(streamers).pipe(
        Effect.map((serialized) => c.json({ streamers: serialized })),
      ),
    ),
  );

  // Channel list for the homebridge-stream-triggers Homebridge plugin: one
  // switch per streamer, highest-priority tvOS-launchable platform wins.
  app.get("/api/trigger-channels", (c) =>
    c.json({ channels: toTriggerChannels(streamers) }),
  );

  // Viewer metrics history for the streamer detail page: daily peak-viewer
  // buckets (~100 days retained) plus the all-time record.
  app.get(
    "/api/streamers/:id/metrics",
    effectHandler((c) => {
      const id = c.req.param("id") ?? "";
      if (!streamers.some((s) => s.id === id)) {
        return Effect.succeed(c.json({ error: "Unknown streamer" }, 404) as Response);
      }
      return Effect.gen(function* () {
        const metrics = yield* getViewerMetricsEffect(id);
        const platforms = yield* fromSync("read platform viewer metrics", () =>
          getPlatformViewerMetrics(id),
        );
        return c.json({
          dailyBuckets: metrics.dailyBuckets,
          allTimeMax: metrics.allTimeMax,
          allTimeMaxTimestamp: metrics.allTimeMaxTimestamp,
          platforms: platforms.map((platform) => ({
            platform: platform.platform,
            username: platform.username,
            dailyBuckets: platform.dailyBuckets,
            allTimeMax: platform.allTimeMax,
            allTimeMaxTimestamp: platform.allTimeMaxTimestamp,
          })),
        }) as Response;
      });
    }),
  );

  // Completed live sessions for the streamer detail page, newest first.
  app.get(
    "/api/streamers/:id/sessions",
    effectHandler((c) => {
      const id = c.req.param("id") ?? "";
      if (!streamers.some((s) => s.id === id)) {
        return Effect.succeed(c.json({ error: "Unknown streamer" }, 404) as Response);
      }
      return fromSync("read stream sessions", () => getStreamSessions(id)).pipe(
        Effect.map(
          ({ sessions }) => c.json({ sessions: [...sessions].reverse() }) as Response,
        ),
      );
    }),
  );

  app.get(
    "/api/streamers/:id/intelligence-details",
    effectHandler((c) => {
      const id = c.req.param("id") ?? "";
      if (!streamers.some((streamer) => streamer.id === id)) {
        return Effect.succeed(c.json({ error: "Unknown streamer" }, 404) as Response);
      }
      const limitParam = Number(c.req.query("limit"));
      const limit = Number.isInteger(limitParam) && limitParam > 0 ? limitParam : 100;
      return Effect.gen(function* () {
        const details = yield* fromSync("read livestream intelligence details", () => ({
          intelligence: getLivestreamIntelligence(id) ?? null,
          diagnostics: getLivestreamDiagnostics(id) ?? null,
          events: getLivestreamEvents(id, limit),
          runtime: livestreamDiagnostics?.getRuntimeDiagnostics() ?? null,
        }));
        return c.json({
          ...details,
          generatedAt: yield* Clock.currentTimeMillis,
        }) as Response;
      });
    }),
  );

  const livestreamFeedbackSchema = Schema.Struct({
    alertId: Schema.String.check(Schema.isUUID()),
    verdict: Schema.Literals(["useful", "not_useful", "false_positive"]),
    note: Schema.optional(Schema.String.check(Schema.isMaxLength(500))),
  });

  app.post(
    "/api/streamers/:id/intelligence-feedback",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const id = c.req.param("id") ?? "";
            if (!streamers.some((streamer) => streamer.id === id)) {
              return c.json({ error: "Unknown streamer" }, 404);
            }
            const parsed = Schema.decodeUnknownExit(livestreamFeedbackSchema)(body);
            if (parsed._tag === "Failure") {
              return c.json({ error: String(parsed.cause) }, 400);
            }
            const feedback = recordLivestreamFeedback({
              streamerId: id,
              ...parsed.value,
            });
            if (!feedback) {
              return c.json({ error: "Alert no longer exists" }, 404);
            }
            return c.json({ feedback }, 201);
          }),
        ),
      ),
    ),
  );

  // Full dashboard state in one payload; also the polling fallback when the
  // SSE stream is unavailable.
  app.get(
    "/api/snapshot",
    effectHandler((c) =>
      buildSnapshot().pipe(Effect.map((snapshot) => c.json(snapshot))),
    ),
  );

  // Realtime dashboard updates. The snapshot is built and serialized once per
  // bus event (debounced to coalesce bursts) and fanned out to every connected
  // client; per-client writes are chained so a slow consumer can't interleave
  // SSE frames. Identical consecutive payloads are skipped.
  interface SseFrame {
    data: string;
    event?: string;
    id?: string;
    retry?: number;
  }
  interface SseClient {
    readonly frames: Queue.Queue<SseFrame>;
  }
  const clients = new Set<SseClient>();
  let lastBroadcast: string | undefined;
  let snapshotEventId = 0;
  const broadcastEffect = buildSnapshot().pipe(
    Effect.map(JSON.stringify),
    Effect.tap((payload) =>
      Effect.sync(() => {
        if (payload === lastBroadcast) return;
        lastBroadcast = payload;
        const frame = {
          event: "snapshot",
          data: payload,
          id: String(snapshotEventId++),
        };
        for (const client of clients) Queue.offerUnsafe(client.frames, frame);
      }),
    ),
    Effect.asVoid,
  );

  app.get("/api/events", (c) => {
    // nginx-family proxies honor this and pass the stream through unbuffered.
    c.header("X-Accel-Buffering", "no");
    return streamSSE(c, (stream) =>
      runPromise(
        Effect.scoped(
          Effect.gen(function* () {
            const frames = yield* Queue.unbounded<SseFrame>();
            const changes = yield* Queue.unbounded<void>();
            const client: SseClient = { frames };
            clients.add(client);
            const unsubscribeClient = taskRunBus.subscribe(() => {
              Queue.offerUnsafe(changes, undefined);
            });
            yield* Effect.addFinalizer(() =>
              Effect.sync(() => {
                unsubscribeClient();
                clients.delete(client);
              }).pipe(
                Effect.andThen(Queue.shutdown(frames)),
                Effect.andThen(Queue.shutdown(changes)),
              ),
            );

            yield* Stream.fromQueue(changes).pipe(
              Stream.debounce(Duration.millis(SSE_DEBOUNCE_MS)),
              Stream.runForEach(() => broadcastEffect),
              Effect.forkScoped,
            );
            yield* Effect.repeat(
              Clock.currentTimeMillis.pipe(
                Effect.tap((now) =>
                  Effect.sync(() =>
                    Queue.offerUnsafe(frames, {
                      event: "ping",
                      data: String(now),
                    }),
                  ),
                ),
              ),
              { schedule: Schedule.spaced(Duration.millis(SSE_HEARTBEAT_MS)) },
            ).pipe(Effect.forkScoped);
            const writer = yield* Effect.forever(
              Queue.take(frames).pipe(
                Effect.flatMap((frame) =>
                  fromPromise("write dashboard SSE frame", () =>
                    stream.writeSSE(frame),
                  ),
                ),
              ),
            ).pipe(Effect.forkScoped);

            const initialSnapshot = yield* buildSnapshot();
            Queue.offerUnsafe(frames, {
              event: "snapshot",
              data: JSON.stringify(initialSnapshot),
              id: String(snapshotEventId++),
            });
            yield* awaitSseWriter(
              writer,
              Effect.callback<void>((resume) => {
                stream.onAbort(() => resume(Effect.void));
              }),
            );
          }),
        ),
      ),
    );
  });

  app.get("/api/data/entities", (c) => {
    const entities = listManagedEntities();
    return c.json({
      entities,
      storage: getManagedDataSummary(entities),
    });
  });

  app.get("/api/data/entities/:slug", (c) => {
    const data = getManagedEntity(c.req.param("slug"));
    if (!data) return c.json({ error: "Unknown entity" }, 404);
    return c.json(data);
  });

  app.delete(
    "/api/data/entities/:slug",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.flatMap((body) =>
            Effect.gen(function* () {
              if (
                typeof body !== "object" ||
                body === null ||
                !("key" in body) ||
                typeof body.key !== "object" ||
                body.key === null ||
                Array.isArray(body.key)
              ) {
                return c.json(
                  { error: "A primary key object is required" },
                  400,
                ) as Response;
              }
              const result = deleteManagedEntityRow(
                c.req.param("slug") ?? "",
                body.key as Record<string, unknown>,
              );
              if (!result) return c.json({ error: "Unknown entity" }, 404) as Response;
              switch (result.status) {
                case "invalid-key":
                  return c.json(
                    { error: "The primary key does not match this entity" },
                    400,
                  ) as Response;
                case "not-found":
                  return c.json({ error: "Row not found" }, 404) as Response;
                case "blocked":
                  return c.json({ error: result.reason }, 409) as Response;
                case "deleted":
                  logger.info(
                    `Deleted row from "${c.req.param("slug")}"`,
                    body.key as Record<string, unknown>,
                  );
                  yield* broadcastEffect;
                  return c.json({ deleted: true }) as Response;
              }
            }),
          ),
        ),
      ),
    ),
  );

  app.post("/api/tasks/:name/run", (c) => {
    const name = c.req.param("name");
    try {
      const { runId } = registry.runNow(name);
      logger.info(`Manual run requested for "${name}"`);
      return c.json({ runId }, 202);
    } catch (error) {
      return taskRunErrorResponse(c, error);
    }
  });

  const recommendationRunSchema = runRequestSchema(MAX_RECOMMENDATIONS_PER_RUN);

  app.post(
    "/api/recommendations/run",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const parsed = Schema.decodeUnknownExit(recommendationRunSchema)(body);
            if (parsed._tag === "Failure") {
              return c.json(
                { error: runRequestError(MAX_RECOMMENDATIONS_PER_RUN) },
                400,
              );
            }
            try {
              const { runId } = registry.runNow("Recommendations", parsed.value);
              logger.info(
                `Manual recommendation run requested for up to ${parsed.value.maxRecommendations} item(s)`,
              );
              return c.json({ runId }, 202);
            } catch (error) {
              return taskRunErrorResponse(c, error);
            }
          }),
        ),
      ),
    ),
  );

  app.get("/api/task-runs", (c) => {
    const task = c.req.query("task");
    const limitParam = Number(c.req.query("limit"));
    const limit =
      Number.isInteger(limitParam) && limitParam > 0 ? Math.min(limitParam, 200) : 50;
    return c.json({ runs: getRuns(task || undefined, limit).map(serializeRun) });
  });

  app.get("/api/costs", (c) => {
    const value = c.req.query("days") ?? "30";
    const days = value === "all" ? null : Number(value);
    if (days !== null && ![7, 30, 90].includes(days)) {
      return c.json({ error: "days must be 7, 30, 90, or all" }, 400);
    }
    return c.json(summarizeCosts(getCostEvents(), { days, timeZone: config.TZ }));
  });

  // In-flight runs read from the live capture buffer, finished runs from the
  // persisted row (absent when the run logged nothing or predates capture).
  const collectRunLogs = (runId: string) =>
    getActiveRunLogs(runId) ?? getRunLogs(runId) ?? { lines: [], dropped: 0 };

  app.get("/api/task-runs/:runId/logs", (c) => {
    const runId = c.req.param("runId");
    const run = getRun(runId);
    if (!run) return c.json({ error: "Unknown run" }, 404);
    const logs = collectRunLogs(runId);
    return c.json({ run: serializeRun(run), lines: logs.lines, dropped: logs.dropped });
  });

  // Live log tail for one run, opened on demand while a log viewer is up:
  // an "init" frame replaying what's buffered so far, "line" frames as the
  // task logs, and a "done" frame carrying the settled run. For finished runs
  // init and done arrive back to back. Reconnects are safe: init re-sends the
  // full buffer and the client replaces (not appends) its state.
  app.get("/api/task-runs/:runId/logs/stream", (c) => {
    const runId = c.req.param("runId");
    const run = getRun(runId);
    if (!run) return c.json({ error: "Unknown run" }, 404);
    c.header("X-Accel-Buffering", "no");
    return streamSSE(c, (stream) =>
      runPromise(
        Effect.scoped(
          Effect.gen(function* () {
            interface LogFrame {
              readonly frame: SseFrame;
              readonly written?: Deferred.Deferred<void>;
            }
            const frames = yield* Queue.unbounded<LogFrame>();
            const finished = yield* Deferred.make<void>();
            let eventId = 0;
            let completionQueued = false;
            const sendDone = () => {
              if (completionQueued) return;
              completionQueued = true;
              Queue.offerUnsafe(frames, {
                frame: {
                  event: "done",
                  data: JSON.stringify(serializeRun(getRun(runId) ?? run)),
                  id: String(eventId++),
                },
                written: finished,
              });
            };
            const unsubscribeLogs = runLogBus.subscribe((event) => {
              if (event.runId !== runId) return;
              if (event.type === "line") {
                Queue.offerUnsafe(frames, {
                  frame: {
                    event: "line",
                    data: JSON.stringify(event.line),
                    id: String(eventId++),
                  },
                });
              } else {
                sendDone();
              }
            });
            yield* Effect.addFinalizer(() =>
              Effect.sync(unsubscribeLogs).pipe(Effect.andThen(Queue.shutdown(frames))),
            );

            const logs = collectRunLogs(runId);
            Queue.offerUnsafe(frames, {
              frame: {
                event: "init",
                data: JSON.stringify({
                  run: serializeRun(run),
                  lines: logs.lines,
                  dropped: logs.dropped,
                }),
                id: String(eventId++),
              },
            });

            const writer = yield* Effect.forever(
              Queue.take(frames).pipe(
                Effect.flatMap(({ frame, written }) =>
                  fromPromise("write task log SSE frame", () =>
                    stream.writeSSE(frame),
                  ).pipe(
                    Effect.tap(() =>
                      written ? Deferred.succeed(written, undefined) : Effect.void,
                    ),
                  ),
                ),
              ),
            ).pipe(Effect.forkScoped);
            yield* Effect.repeat(
              Clock.currentTimeMillis.pipe(
                Effect.tap((now) =>
                  Effect.sync(() =>
                    Queue.offerUnsafe(frames, {
                      frame: { event: "ping", data: String(now) },
                    }),
                  ),
                ),
              ),
              { schedule: Schedule.spaced(Duration.millis(SSE_HEARTBEAT_MS)) },
            ).pipe(Effect.forkScoped);

            if (run.status !== "running") sendDone();
            yield* awaitSseWriter(
              writer,
              Effect.raceFirst(
                Deferred.await(finished),
                Effect.callback<void>((resume) => {
                  stream.onAbort(() => resume(Effect.void));
                }),
              ),
            );
          }),
        ),
      ),
    );
  });

  app.get("/api/recommendations", (c) => {
    const recommendations = getAllRecommendations().map(serializeRecommendation);
    return c.json({ recommendations });
  });

  app.get("/api/recommendations/taste-profile", (c) =>
    c.json({ profile: getLatestTasteProfile() ?? null }),
  );

  // Registered after /taste-profile so the static route keeps precedence.
  app.get("/api/recommendations/:id", (c) => {
    const recommendation = getRecommendation(c.req.param("id"));
    if (!recommendation) return c.json({ error: "Recommendation not found" }, 404);
    return c.json({ recommendation: serializeRecommendation(recommendation) });
  });

  app.post(
    "/api/recommendations/:id/feedback",
    feedbackRoute({
      schema: Schema.Struct({
        feedback: Schema.optional(
          Schema.Literals(["good_pick", "not_for_me", "already_watched"]),
        ),
        note: Schema.optional(Schema.String.check(Schema.isMaxLength(1000))),
      }),
      get: getRecommendation,
      setFeedback: setRecommendationFeedback,
      serialize: serializeRecommendation,
    }),
  );

  app.get("/api/podcast-recommendations", (c) => {
    const recommendations = getAllPodcastRecommendations().map(
      serializePodcastRecommendation,
    );
    return c.json({ recommendations });
  });

  // Registered before /:id so the static route keeps precedence.
  app.get("/api/podcast-recommendations/taste-profile", (c) =>
    c.json({ profile: getLatestPodcastTasteProfile() ?? null }),
  );

  app.get("/api/podcast-recommendations/:id", (c) => {
    const recommendation = getPodcastRecommendation(c.req.param("id"));
    if (!recommendation) return c.json({ error: "Recommendation not found" }, 404);
    return c.json({ recommendation: serializePodcastRecommendation(recommendation) });
  });

  app.post(
    "/api/podcast-recommendations/:id/feedback",
    feedbackRoute({
      schema: Schema.Struct({
        feedback: Schema.optional(Schema.Literals(["good_pick", "not_for_me"])),
        note: Schema.optional(Schema.String.check(Schema.isMaxLength(1000))),
      }),
      get: getPodcastRecommendation,
      setFeedback: setPodcastRecommendationFeedback,
      serialize: serializePodcastRecommendation,
    }),
  );

  const podcastRunSchema = runRequestSchema(MAX_PODCAST_RECOMMENDATIONS_PER_RUN);

  app.post(
    "/api/podcast-recommendations/run",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const parsed = Schema.decodeUnknownExit(podcastRunSchema)(body);
            if (parsed._tag === "Failure") {
              return c.json(
                { error: runRequestError(MAX_PODCAST_RECOMMENDATIONS_PER_RUN) },
                400,
              );
            }
            try {
              const { runId } = registry.runNow("PodcastRecs", parsed.value);
              logger.info(
                `Manual podcast recommendation run requested for up to ${parsed.value.maxRecommendations} episode(s)`,
              );
              return c.json({ runId }, 202);
            } catch (error) {
              return taskRunErrorResponse(c, error);
            }
          }),
        ),
      ),
    ),
  );

  // Per-email outcomes recorded by the parcel and calendar pipelines,
  // newest first.
  app.get("/api/email-activity", (c) => {
    const pipelineParam = c.req.query("pipeline");
    if (
      pipelineParam !== undefined &&
      pipelineParam !== "ParcelTracker" &&
      pipelineParam !== "CalendarEvents"
    ) {
      return c.json({ error: "Unknown pipeline" }, 400);
    }
    const pipeline = pipelineParam as EmailPipelineName | undefined;
    const limitParam = Number(c.req.query("limit") ?? 100);
    const limit = Number.isFinite(limitParam)
      ? Math.min(Math.max(1, Math.floor(limitParam)), KEEP_PER_PIPELINE * 2)
      : 100;
    const activities = getRecentEmailActivity(pipeline, limit).map(
      serializeEmailActivity,
    );
    return c.json({ activities });
  });

  // Captured log lines for one email's processing phase. Filtered/skipped
  // emails never reach processing, so they legitimately have no lines.
  app.get("/api/email-activity/:activityId/logs", (c) => {
    const activityId = c.req.param("activityId");
    const activity = getEmailActivity(activityId);
    if (!activity) return c.json({ error: "Unknown activity" }, 404);
    const logs = getEmailActivityLogs(activityId);
    return c.json({
      activity: serializeEmailActivity(activity),
      lines: logs?.lines ?? [],
      dropped: logs?.dropped ?? 0,
    });
  });

  // Re-fetch the email from the mail server and run it through its pipeline again.
  // Dedup gates make this safe: anything that already landed is skipped.
  app.post(
    "/api/email-activity/:activityId/reprocess",
    effectHandler((c) =>
      Effect.gen(function* () {
        const activityId = c.req.param("activityId") ?? "";
        const activity = getEmailActivity(activityId);
        if (!activity) {
          return c.json({ error: "Unknown activity" }, 404) as Response;
        }
        const { transport, handlers } = emailControls;
        const handler = handlers?.get(activity.pipeline);
        if (!transport || !handler) {
          return c.json({ error: "Email pipelines are not active" }, 503) as Response;
        }
        const email = yield* transport.fetchEmailByIdEffect(activity.emailId);
        if (!email) {
          return c.json(
            { error: "Email no longer exists in the mailbox" },
            404,
          ) as Response;
        }
        logger.info(`Reprocessing "${activity.subject}" through ${activity.pipeline}`);
        // Clear only after the handler succeeds, preserving durable retries on failure.
        yield* handler.handleEmailsEffect([email]);
        yield* Effect.sync(() => clearEmailRetry(activity.pipeline, activity.emailId));
        const updated = getEmailActivity(activityId) ?? activity;
        return c.json({ activity: serializeEmailActivity(updated) }) as Response;
      }),
    ),
  );

  // User-editable sender rules plus the read-only built-in lists, so the UI
  // can show everything the filters consult. User allow rules override the
  // built-in blocklists; built-ins live in code (version-controlled, survive
  // DB resets) and are not seeded into the entity.
  app.get("/api/email-rules", (c) => {
    return c.json({
      rules: listEmailRules(),
      builtin: {
        parcel: {
          blocked: PARCEL_BUILTIN_BLOCKED,
          autoPass: PARCEL_BUILTIN_AUTO_PASS,
        },
        calendar: {
          blocked: CALENDAR_BUILTIN_BLOCKED,
          autoPass: CALENDAR_BUILTIN_AUTO_PASS,
        },
      },
    });
  });

  const emailRuleSchema = Schema.Struct({
    pattern: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200)),
    scope: Schema.Literals(["parcel", "calendar", "both"]),
    verdict: Schema.Literals(["block", "allow"]),
  });

  app.post(
    "/api/email-rules",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const parsed = Schema.decodeUnknownExit(emailRuleSchema)(body);
            if (parsed._tag === "Failure") {
              return c.json({ error: "pattern, scope, and verdict are required" }, 400);
            }
            const pattern = normalizeRulePattern(parsed.value.pattern);
            const scope = parsed.value.scope as EmailRuleScope;
            const verdict = parsed.value.verdict as EmailRuleVerdict;
            if (!pattern) {
              return c.json({ error: "pattern, scope, and verdict are required" }, 400);
            }

            // A block rule a built-in list already covers is redundant — surface that
            // rather than silently storing a no-op user rule. (Allow rules are the
            // escape hatch from built-ins, so they're never rejected this way.)
            if (verdict === "block" && matchesBuiltinBlock(pattern, scope)) {
              return c.json(
                { status: "builtin", message: "Already blocked by a built-in list" },
                200,
              );
            }

            const result = upsertEmailRuleChecked({ pattern, scope, verdict });
            const status = result.alreadyExists
              ? "exists"
              : result.merged
                ? "merged"
                : "created";
            logger.info(
              `Email rule ${status}: ${result.rule.verdict} ${result.rule.pattern} (${result.rule.scope})`,
            );
            return c.json(
              { rule: result.rule, status },
              status === "created" ? 201 : 200,
            );
          }),
        ),
      ),
    ),
  );

  app.delete("/api/email-rules/:ruleId", (c) => {
    const deleted = deleteEmailRule(c.req.param("ruleId"));
    if (!deleted) return c.json({ error: "Unknown rule" }, 404);
    return c.json({ deleted: true });
  });

  // Explicit user feedback on an email's outcome; feeds triage corrections.
  app.post(
    "/api/email-activity/:activityId/feedback",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const activity = getEmailActivity(c.req.param("activityId") ?? "");
            if (!activity) return c.json({ error: "Unknown activity" }, 404);
            const parsed = Schema.decodeUnknownExit(
              Schema.Struct({
                verdict: Schema.NullOr(Schema.Literals(["not_relevant", "missed"])),
                note: Schema.optional(Schema.String.check(Schema.isMaxLength(500))),
              }),
            )(body);
            if (parsed._tag === "Failure") {
              return c.json(
                { error: "A verdict (not_relevant | missed | null) is required" },
                400,
              );
            }
            if (parsed.value.verdict === null) {
              deleteEmailFeedback(activity.activityId);
              return c.json({ feedback: null });
            }
            const feedback = recordEmailFeedback({
              pipeline: activity.pipeline,
              emailId: activity.emailId,
              subject: activity.subject,
              from: activity.from,
              verdict: parsed.value.verdict as EmailFeedbackVerdict,
              note: parsed.value.note,
            });
            logger.info(
              `Email feedback: ${feedback.verdict} for "${activity.subject}" (${activity.pipeline})`,
            );
            return c.json({ feedback });
          }),
        ),
      ),
    ),
  );

  app.get("/api/email-feedback", (c) => {
    return c.json({ feedback: listEmailFeedback() });
  });

  // Forget a submitted tracking number so a future email can resubmit it
  // (escape hatch for the permanent dedup gate after a mis-extraction).
  app.delete("/api/parcel-tracker/deliveries/:trackingNumber", (c) => {
    const trackingNumber = c.req.param("trackingNumber");
    const deleted = SubmittedDeliveryEntity.delete({ trackingNumber });
    if (!deleted) return c.json({ error: "Unknown tracking number" }, 404);
    logger.info(`Forgot submitted delivery ${trackingNumber}`);
    return c.json({ deleted: true });
  });

  // Stored briefing history (last 50 notifications per briefing), one row per
  // briefing name; notifications are returned newest-first.
  app.get("/api/briefings", (c) => {
    const briefings = getAllBriefingHistories()
      .map((history) => ({
        name: history.briefingName,
        notifications: history.notifications
          .map((n) => ({
            title: n.title,
            message: n.message,
            url: n.url,
            timestamp: n.timestamp,
            runId: n.runId ?? null,
            costCents: n.costCents ?? null,
          }))
          .sort((a, b) => b.timestamp - a.timestamp),
      }))
      .sort(
        (a, b) =>
          (b.notifications[0]?.timestamp ?? 0) - (a.notifications[0]?.timestamp ?? 0),
      );
    return c.json({ briefings });
  });

  app.get("/api/workspaces", (c) => {
    return c.json({
      workspaces: workspaceDefinitions.map((definition) => {
        const subjects = listWorkspaceSubjects(definition.id);
        const actions = listWorkspaceActions(definition.id);
        return {
          ...definition,
          subjects,
          activeSubjectCount: subjects.filter((subject) => subject.status === "active")
            .length,
          pendingActionCount: actions.filter((action) => action.status === "pending")
            .length,
          openPapercutCount: listWorkspacePapercuts(definition.id, "open").length,
        };
      }),
    });
  });

  app.get("/api/workspaces/:workspaceId", (c) => {
    const definition = getWorkspaceDefinition(c.req.param("workspaceId") ?? "");
    if (!definition) return c.json({ error: "Unknown workspace" }, 404);
    return c.json({
      workspace: definition,
      subjects: listWorkspaceSubjects(definition.id),
      actions: listWorkspaceActions(definition.id),
      papercuts: listWorkspacePapercuts(definition.id, "open"),
    });
  });

  app.get("/api/workspaces/:workspaceId/subjects/:subjectId", (c) => {
    const workspaceId = c.req.param("workspaceId") ?? "";
    const subjectId = c.req.param("subjectId") ?? "";
    const definition = getWorkspaceDefinition(workspaceId);
    const subject = getWorkspaceSubject(workspaceId, subjectId);
    if (!definition || !subject)
      return c.json({ error: "Unknown workspace subject" }, 404);
    return c.json({
      workspace: definition,
      subject,
      artifacts: getLatestWorkspaceArtifacts(workspaceId, subjectId),
      artifactRevisions: listWorkspaceArtifactRevisions(workspaceId, subjectId),
      messages: listWorkspaceMessages(workspaceId, subjectId),
      sources: listWorkspaceSources(workspaceId, subjectId),
      actions: listWorkspaceActions(workspaceId, subjectId),
      emailScope: serializeWorkspaceEmailScope(
        getWorkspaceEmailScope(workspaceId, subjectId),
      ),
      papercuts: listWorkspacePapercuts(workspaceId, "open").filter(
        (papercut) => !papercut.subjectId || papercut.subjectId === subjectId,
      ),
    });
  });

  const workspaceMessageSchema = Schema.Struct({
    message: Schema.String.check(
      Schema.isTrimmed(),
      Schema.isMinLength(1),
      Schema.isMaxLength(20_000),
    ),
    subjectId: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
  });
  app.post(
    "/api/workspaces/:workspaceId/messages",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const definition = getWorkspaceDefinition(c.req.param("workspaceId") ?? "");
            if (!definition) return c.json({ error: "Unknown workspace" }, 404);
            const parsed = Schema.decodeUnknownExit(workspaceMessageSchema)(body);
            if (parsed._tag === "Failure")
              return c.json({ error: "A message is required" }, 400);
            if (
              parsed.value.subjectId &&
              !getWorkspaceSubject(definition.id, parsed.value.subjectId)
            ) {
              return c.json({ error: "Unknown workspace subject" }, 404);
            }
            try {
              const run = registry.runNow(definition.taskName, parsed.value);
              return c.json({ runId: run.runId }, 202);
            } catch (error) {
              if (error instanceof TaskAlreadyRunningError) {
                return c.json({ error: "Workspace agent is already running" }, 409);
              }
              if (error instanceof TaskNotFoundError) {
                return c.json({ error: "Workspace task is unavailable" }, 503);
              }
              throw error;
            }
          }),
        ),
      ),
    ),
  );

  const subjectStatusSchema = Schema.Struct({
    status: Schema.Literals(["active", "paused", "completed", "archived"]),
  });
  app.post(
    "/api/workspaces/:workspaceId/subjects/:subjectId/status",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const workspaceId = c.req.param("workspaceId") ?? "";
            const subjectId = c.req.param("subjectId") ?? "";
            const subject = getWorkspaceSubject(workspaceId, subjectId);
            if (!subject) return c.json({ error: "Unknown workspace subject" }, 404);
            const parsed = Schema.decodeUnknownExit(subjectStatusSchema)(body);
            if (parsed._tag === "Failure")
              return c.json({ error: "A valid status is required" }, 400);
            return c.json({
              subject: upsertWorkspaceSubject({
                workspaceId: subject.workspaceId,
                subjectId: subject.subjectId,
                title: subject.title,
                status: parsed.value.status,
                summary: subject.summary,
                createdAt: subject.createdAt,
                lastResearchedAt: subject.lastResearchedAt,
              }),
            });
          }),
        ),
      ),
    ),
  );

  app.post(
    "/api/workspace-actions/:actionId/approve",
    effectHandler((c) =>
      approveWorkspaceActionEffect(c.req.param("actionId") ?? "", logger).pipe(
        Effect.map((action): Response => c.json({ action })),
        Effect.catch((error) =>
          Effect.succeed<Response>(
            c.json(
              { error: error instanceof Error ? error.message : "Action failed" },
              409,
            ),
          ),
        ),
      ),
    ),
  );

  app.post(
    "/api/workspace-actions/:actionId/reject",
    effectHandler((c) =>
      rejectWorkspaceActionEffect(c.req.param("actionId") ?? "").pipe(
        Effect.map((action): Response => c.json({ action })),
        Effect.catch((error) =>
          Effect.succeed<Response>(
            c.json(
              { error: error instanceof Error ? error.message : "Action failed" },
              409,
            ),
          ),
        ),
      ),
    ),
  );

  app.get("/api/workspace-papercuts", (c) => {
    const status = c.req.query("status");
    const parsed = Schema.decodeUnknownExit(
      Schema.Literals(["open", "addressed", "dismissed"]),
    )(status);
    return c.json({
      papercuts: listWorkspacePapercuts(
        c.req.query("workspaceId"),
        parsed._tag === "Success" ? parsed.value : undefined,
      ),
    });
  });

  app.post(
    "/api/workspace-papercuts/:papercutId/resolve",
    effectHandler((c) =>
      rejectOversizedJson(
        c,
        requestJsonEffect(c).pipe(
          Effect.map((body): Response => {
            const parsed = Schema.decodeUnknownExit(
              Schema.Struct({
                status: Schema.Literals(["addressed", "dismissed"]),
                resolution: Schema.String.check(
                  Schema.isTrimmed(),
                  Schema.isMinLength(1),
                  Schema.isMaxLength(2_000),
                ),
              }),
            )(body);
            if (parsed._tag === "Failure")
              return c.json({ error: "Status and resolution are required" }, 400);
            const papercut = resolveWorkspacePapercut(
              c.req.param("papercutId") ?? "",
              parsed.value.status,
              parsed.value.resolution,
            );
            if (!papercut) return c.json({ error: "Unknown papercut" }, 404);
            return c.json({ papercut });
          }),
        ),
      ),
    ),
  );

  app.get("/api/health", (c) => c.json({ status: "ok" }));

  app.get(
    "/api/pets",
    effectHandler((c) =>
      Effect.sync((): Response => {
        const pets = getAllPetsWithHistory();
        const response = pets.map((pet) => ({
          petId: pet.pet_id,
          name: pet.name,
          currentWeight: round(pet.current_weight),
          weightHistory: pet.weightHistory.map((entry) => ({
            timestamp: entry.timestamp,
            weight: round(entry.weight),
          })),
          dailyVisits: getDailyVisitCounts(pet.pet_id),
        }));
        return c.json(response);
      }),
    ),
  );

  app.get("/api/pets/:petId/export.csv", (c) => {
    const petId = c.req.param("petId");
    const daysParam = c.req.query("days");

    let history = getWeightHistory(petId);
    if (daysParam) {
      const days = Number(daysParam);
      if (!Number.isNaN(days) && days > 0) {
        const cutoff = new Date();
        cutoff.setDate(cutoff.getDate() - days);
        history = history.filter((r) => new Date(r.timestamp) >= cutoff);
      }
    }

    const lines = ["timestamp,weight_lbs"];
    for (const r of history) {
      lines.push(`${r.timestamp},${r.weight}`);
    }

    c.header("Content-Type", "text/csv");
    const pet = getPet(petId);
    const filename = pet
      ? `${pet.name.toLowerCase()}-weight.csv`
      : `${petId}-weight.csv`;
    c.header("Content-Disposition", `attachment; filename="${filename}"`);
    return c.body(lines.join("\n"));
  });

  registerPressPodsRoutes(app, registry, logger);

  // Vite content-hashes asset filenames, so they can be cached forever; the
  // HTML must revalidate so deploys pick up new asset hashes.
  app.use(
    "*",
    effectMiddleware((c, next) =>
      next.pipe(
        Effect.tap(() =>
          Effect.sync(() => {
            if (c.req.path.startsWith("/assets/")) {
              c.res.headers.set("Cache-Control", "public, max-age=31536000, immutable");
            } else if (c.res.headers.get("Content-Type")?.includes("text/html")) {
              c.res.headers.set("Cache-Control", "no-cache");
            }
          }),
        ),
      ),
    ),
  );
  app.use("*", serveStatic({ root: "./frontend/dist" }));
  app.use("*", serveStatic({ root: "./frontend/dist", path: "index.html" }));

  const server = serve({ fetch: app.fetch, port }, () => {
    logger.info(`Server listening on port ${port}`);
  });

  const closeEffect = Effect.gen(function* () {
    if (mcp) {
      yield* fromPromise("close MCP server", () => mcp.close());
    }
    const closed = Effect.callback<void, IntegrationError>((resume) => {
      server.close((cause) =>
        resume(
          cause
            ? Effect.fail(
                new IntegrationError({ operation: "close HTTP server", cause }),
              )
            : Effect.void,
        ),
      );
    });
    // Open SSE streams would otherwise keep the process alive indefinitely.
    if ("closeAllConnections" in server) server.closeAllConnections();
    yield* closed;
  });
  return closeEffect;
}
