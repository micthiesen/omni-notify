import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import type { Logger } from "@micthiesen/mitools/logging";
import { type Context, Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { z } from "zod";
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
} from "./jmap/activity.js";
import { getEmailActivityLogs } from "./jmap/activityLogs.js";
import type { JmapContext } from "./jmap/client.js";
import type { EmailHandler } from "./jmap/dispatcher.js";
import { fetchEmailById } from "./jmap/emailFetcher.js";
import {
  deleteEmailFeedback,
  type EmailFeedbackVerdict,
  listEmailFeedback,
  recordEmailFeedback,
} from "./jmap/feedback.js";
import { clearEmailRetry } from "./jmap/retry.js";
import {
  deleteEmailRule,
  type EmailRuleScope,
  type EmailRuleVerdict,
  listEmailRules,
  normalizeRulePattern,
  upsertEmailRuleChecked,
} from "./jmap/senderRules.js";
import { getViewerMetrics } from "./live-check/metrics/persistence.js";
import { getStreamerStatus } from "./live-check/persistence.js";
import { platformConfigs } from "./live-check/platforms/index.js";
import { getStreamSessions } from "./live-check/sessions.js";
import type { PlatformBinding, Streamer } from "./live-check/streamers.js";
import { toTriggerChannels } from "./live-check/triggerChannels.js";
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
import {
  getAllRecommendations,
  getRecommendation,
  type RecommendationData,
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
    url: platformConfigs[binding.platform].getLiveUrl(binding.username),
  };
}

function serializeStreamer(streamer: Streamer) {
  const status = getStreamerStatus(streamer.id);
  const base = {
    id: streamer.id,
    displayName: streamer.displayName,
    bindings: streamer.bindings.map(serializeBinding),
  };
  if (status.isLive) {
    return {
      ...base,
      live: true as const,
      title: status.primaryTitle,
      startedAt: toEpochMs(status.startedAt),
      maxViewerCount: status.maxViewerCount,
      primary: serializeBinding(status.primary),
    };
  }
  return {
    ...base,
    live: false as const,
    lastStartedAt: status.lastStartedAt ? toEpochMs(status.lastStartedAt) : null,
    lastEndedAt: status.lastEndedAt ? toEpochMs(status.lastEndedAt) : null,
    lastMaxViewerCount: status.lastMaxViewerCount ?? null,
  };
}

const SNAPSHOT_RUN_LIMIT = 30;
const SSE_DEBOUNCE_MS = 150;
const SSE_HEARTBEAT_MS = 25_000;

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
  z.object({ maxRecommendations: z.number().int().min(1).max(max) });
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
  schema: z.ZodType<{ feedback?: TFeedback; note?: string }>;
  get: (id: string) => TData | undefined;
  setFeedback: (
    id: string,
    input: { feedback?: TFeedback; note?: string },
  ) => TData | undefined;
  serialize: (data: TData) => unknown;
}) {
  return async (c: Context) => {
    const parsed = options.schema.safeParse(await c.req.json().catch(() => null));
    if (!parsed.success) {
      return c.json({ error: "Invalid recommendation feedback" }, 400);
    }
    // A rating, a free-form note, or both — but at least one must be present.
    if (parsed.data.feedback === undefined && !parsed.data.note?.trim()) {
      return c.json({ error: "A rating or a note is required" }, 400);
    }
    // Both routes bind :id, but the generic Context can't prove it.
    const id = c.req.param("id") ?? "";
    const existing = options.get(id);
    if (!existing) return c.json({ error: "Recommendation not found" }, 404);
    if (existing.status === "pending" || existing.status === "failed") {
      return c.json({ error: "Undelivered recommendations cannot be rated" }, 409);
    }
    const recommendation = options.setFeedback(id, {
      feedback: parsed.data.feedback,
      note: parsed.data.note?.trim() || undefined,
    });
    if (!recommendation) return c.json({ error: "Recommendation not found" }, 404);
    return c.json({ recommendation: options.serialize(recommendation) });
  };
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
 * in by index.ts after the JMAP features start; empty in server-only mode.
 */
export interface EmailControls {
  ctx?: JmapContext;
  handlers?: Map<string, EmailHandler>;
}

export function startServer(
  port: number,
  parentLogger: Logger,
  registry: TaskRegistry,
  streamers: Streamer[],
  emailControls: EmailControls = {},
): () => void {
  const logger = parentLogger.extend("Server");
  const app = new Hono();

  app.use("/api/*", async (c, next) => {
    if (c.req.method !== "GET" && c.req.method !== "HEAD") {
      const origin = c.req.header("Origin");
      const host = c.req.header("Host");
      let sameOrigin = true;
      if (origin) {
        try {
          sameOrigin = Boolean(host) && new URL(origin).host === host;
        } catch {
          sameOrigin = false;
        }
      }
      if (!sameOrigin) {
        return c.json({ error: "Cross-origin mutations are not allowed" }, 403);
      }
    }
    await next();
    c.header("X-Content-Type-Options", "nosniff");
  });

  const buildSnapshot = () => ({
    tasks: registry.list().map((task) => ({
      ...task,
      lastRun: task.lastRun ? serializeRun(task.lastRun) : null,
    })),
    streamers: streamers.map(serializeStreamer),
    runs: getRuns(undefined, SNAPSHOT_RUN_LIMIT).map(serializeRun),
  });

  app.get("/api/tasks", (c) =>
    c.json({
      tasks: registry.list().map((task) => ({
        ...task,
        lastRun: task.lastRun ? serializeRun(task.lastRun) : null,
      })),
    }),
  );

  app.get("/api/streamers", (c) =>
    c.json({ streamers: streamers.map(serializeStreamer) }),
  );

  // Channel list for the homebridge-stream-triggers Homebridge plugin: one
  // switch per streamer, highest-priority tvOS-launchable platform wins.
  app.get("/api/trigger-channels", (c) =>
    c.json({ channels: toTriggerChannels(streamers) }),
  );

  // Viewer metrics history for the streamer detail page: daily peak-viewer
  // buckets (~100 days retained) plus the all-time record.
  app.get("/api/streamers/:id/metrics", (c) => {
    const id = c.req.param("id");
    if (!streamers.some((s) => s.id === id)) {
      return c.json({ error: "Unknown streamer" }, 404);
    }
    const metrics = getViewerMetrics(id);
    return c.json({
      dailyBuckets: metrics.dailyBuckets,
      allTimeMax: metrics.allTimeMax,
      allTimeMaxTimestamp: metrics.allTimeMaxTimestamp,
    });
  });

  // Completed live sessions for the streamer detail page, newest first.
  app.get("/api/streamers/:id/sessions", (c) => {
    const id = c.req.param("id");
    if (!streamers.some((s) => s.id === id)) {
      return c.json({ error: "Unknown streamer" }, 404);
    }
    const sessions = [...getStreamSessions(id).sessions].reverse();
    return c.json({ sessions });
  });

  // Full dashboard state in one payload; also the polling fallback when the
  // SSE stream is unavailable.
  app.get("/api/snapshot", (c) => c.json(buildSnapshot()));

  // Realtime dashboard updates. The snapshot is built and serialized once per
  // bus event (debounced to coalesce bursts) and fanned out to every connected
  // client; per-client writes are chained so a slow consumer can't interleave
  // SSE frames. Identical consecutive payloads are skipped.
  interface SseClient {
    write(payload: string): void;
    ping(): void;
  }
  const clients = new Set<SseClient>();
  let lastBroadcast: string | undefined;
  let debounce: NodeJS.Timeout | undefined;
  const broadcast = () => {
    const payload = JSON.stringify(buildSnapshot());
    if (payload === lastBroadcast) return;
    lastBroadcast = payload;
    for (const client of clients) client.write(payload);
  };
  const unsubscribe = taskRunBus.subscribe(() => {
    clearTimeout(debounce);
    debounce = setTimeout(broadcast, SSE_DEBOUNCE_MS);
  });
  const heartbeat = setInterval(() => {
    for (const client of clients) client.ping();
  }, SSE_HEARTBEAT_MS);

  app.get("/api/events", (c) => {
    // nginx-family proxies honor this and pass the stream through unbuffered.
    c.header("X-Accel-Buffering", "no");
    return streamSSE(c, async (stream) => {
      let eventId = 0;
      let queue: Promise<void> = Promise.resolve();
      const enqueue = (frame: Parameters<typeof stream.writeSSE>[0]) => {
        queue = queue.then(() => stream.writeSSE(frame)).catch(() => {});
      };
      const client: SseClient = {
        write: (payload) =>
          enqueue({ event: "snapshot", data: payload, id: String(eventId++) }),
        ping: () => enqueue({ event: "ping", data: String(Date.now()) }),
      };
      clients.add(client);
      client.write(JSON.stringify(buildSnapshot()));
      await new Promise<void>((resolve) => {
        stream.onAbort(() => {
          clients.delete(client);
          resolve();
        });
      });
    });
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

  app.delete("/api/data/entities/:slug", async (c) => {
    const body: unknown = await c.req.json().catch(() => null);
    if (
      typeof body !== "object" ||
      body === null ||
      !("key" in body) ||
      typeof body.key !== "object" ||
      body.key === null ||
      Array.isArray(body.key)
    ) {
      return c.json({ error: "A primary key object is required" }, 400);
    }
    const result = deleteManagedEntityRow(
      c.req.param("slug"),
      body.key as Record<string, unknown>,
    );
    if (!result) return c.json({ error: "Unknown entity" }, 404);
    switch (result.status) {
      case "invalid-key":
        return c.json({ error: "The primary key does not match this entity" }, 400);
      case "not-found":
        return c.json({ error: "Row not found" }, 404);
      case "blocked":
        return c.json({ error: result.reason }, 409);
      case "deleted":
        logger.info(
          `Deleted row from "${c.req.param("slug")}"`,
          body.key as Record<string, unknown>,
        );
        broadcast();
        return c.json({ deleted: true });
    }
  });

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

  app.post("/api/recommendations/run", async (c) => {
    const parsed = recommendationRunSchema.safeParse(
      await c.req.json().catch(() => null),
    );
    if (!parsed.success) {
      return c.json({ error: runRequestError(MAX_RECOMMENDATIONS_PER_RUN) }, 400);
    }
    try {
      const { runId } = registry.runNow("Recommendations", parsed.data);
      logger.info(
        `Manual recommendation run requested for up to ${parsed.data.maxRecommendations} item(s)`,
      );
      return c.json({ runId }, 202);
    } catch (error) {
      return taskRunErrorResponse(c, error);
    }
  });

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
    return streamSSE(c, async (stream) => {
      let eventId = 0;
      let queue: Promise<void> = Promise.resolve();
      const enqueue = (frame: Parameters<typeof stream.writeSSE>[0]) => {
        queue = queue.then(() => stream.writeSSE(frame)).catch(() => {});
      };
      let resolveDone!: () => void;
      const done = new Promise<void>((resolve) => {
        resolveDone = resolve;
      });
      const sendDone = () => {
        const settled = getRun(runId) ?? run;
        enqueue({
          event: "done",
          data: JSON.stringify(serializeRun(settled)),
          id: String(eventId++),
        });
        resolveDone();
      };
      const unsubscribe = runLogBus.subscribe((event) => {
        if (event.runId !== runId) return;
        if (event.type === "line") {
          enqueue({
            event: "line",
            data: JSON.stringify(event.line),
            id: String(eventId++),
          });
        } else {
          sendDone();
        }
      });
      // Same synchronous block as the subscribe above, so no line can slip
      // between the snapshot and the subscription.
      const logs = collectRunLogs(runId);
      enqueue({
        event: "init",
        data: JSON.stringify({
          run: serializeRun(run),
          lines: logs.lines,
          dropped: logs.dropped,
        }),
        id: String(eventId++),
      });
      if (run.status !== "running") sendDone();
      const pingTimer = setInterval(
        () => enqueue({ event: "ping", data: String(Date.now()) }),
        SSE_HEARTBEAT_MS,
      );
      stream.onAbort(() => resolveDone());
      await done;
      clearInterval(pingTimer);
      unsubscribe();
      // Flush queued frames (the final "done") before the stream closes.
      await queue;
    });
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
      schema: z.object({
        feedback: z.enum(["good_pick", "not_for_me", "already_watched"]).optional(),
        note: z.string().max(1000).optional(),
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
      schema: z.object({
        feedback: z.enum(["good_pick", "not_for_me"]).optional(),
        note: z.string().max(1000).optional(),
      }),
      get: getPodcastRecommendation,
      setFeedback: setPodcastRecommendationFeedback,
      serialize: serializePodcastRecommendation,
    }),
  );

  const podcastRunSchema = runRequestSchema(MAX_PODCAST_RECOMMENDATIONS_PER_RUN);

  app.post("/api/podcast-recommendations/run", async (c) => {
    const parsed = podcastRunSchema.safeParse(await c.req.json().catch(() => null));
    if (!parsed.success) {
      return c.json(
        { error: runRequestError(MAX_PODCAST_RECOMMENDATIONS_PER_RUN) },
        400,
      );
    }
    try {
      const { runId } = registry.runNow("PodcastRecs", parsed.data);
      logger.info(
        `Manual podcast recommendation run requested for up to ${parsed.data.maxRecommendations} episode(s)`,
      );
      return c.json({ runId }, 202);
    } catch (error) {
      return taskRunErrorResponse(c, error);
    }
  });

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

  // Re-fetch the email from Fastmail and run it through its pipeline again.
  // Dedup gates make this safe: anything that already landed is skipped.
  app.post("/api/email-activity/:activityId/reprocess", async (c) => {
    const activityId = c.req.param("activityId");
    const activity = getEmailActivity(activityId);
    if (!activity) return c.json({ error: "Unknown activity" }, 404);
    const { ctx, handlers } = emailControls;
    const handler = handlers?.get(activity.pipeline);
    if (!ctx || !handler) {
      return c.json({ error: "Email pipelines are not active" }, 503);
    }
    const email = await fetchEmailById(ctx, activity.emailId, logger);
    if (!email) {
      return c.json({ error: "Email no longer exists in the mailbox" }, 404);
    }
    logger.info(`Reprocessing "${activity.subject}" through ${activity.pipeline}`);
    // A queued retry for this email is superseded by the manual run (and
    // clearing it narrows the window for a concurrent duplicate pass).
    clearEmailRetry(activity.pipeline, activity.emailId);
    await handler.handleEmails([email]);
    const updated = getEmailActivity(activityId) ?? activity;
    return c.json({ activity: serializeEmailActivity(updated) });
  });

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

  const emailRuleSchema = z.object({
    pattern: z.string().min(1).max(200),
    scope: z.enum(["parcel", "calendar", "both"]),
    verdict: z.enum(["block", "allow"]),
  });

  app.post("/api/email-rules", async (c) => {
    const body: unknown = await c.req.json().catch(() => null);
    const parsed = emailRuleSchema.safeParse(body);
    if (!parsed.success) {
      return c.json({ error: "pattern, scope, and verdict are required" }, 400);
    }
    const pattern = normalizeRulePattern(parsed.data.pattern);
    const scope = parsed.data.scope as EmailRuleScope;
    const verdict = parsed.data.verdict as EmailRuleVerdict;
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
    return c.json({ rule: result.rule, status }, status === "created" ? 201 : 200);
  });

  app.delete("/api/email-rules/:ruleId", (c) => {
    const deleted = deleteEmailRule(c.req.param("ruleId"));
    if (!deleted) return c.json({ error: "Unknown rule" }, 404);
    return c.json({ deleted: true });
  });

  // Explicit user feedback on an email's outcome; feeds triage corrections.
  app.post("/api/email-activity/:activityId/feedback", async (c) => {
    const activity = getEmailActivity(c.req.param("activityId"));
    if (!activity) return c.json({ error: "Unknown activity" }, 404);
    const body: unknown = await c.req.json().catch(() => null);
    const parsed = z
      .object({
        verdict: z.enum(["not_relevant", "missed"]).nullable(),
        note: z.string().max(500).optional(),
      })
      .safeParse(body);
    if (!parsed.success) {
      return c.json(
        { error: "A verdict (not_relevant | missed | null) is required" },
        400,
      );
    }
    if (parsed.data.verdict === null) {
      deleteEmailFeedback(activity.activityId);
      return c.json({ feedback: null });
    }
    const feedback = recordEmailFeedback({
      pipeline: activity.pipeline,
      emailId: activity.emailId,
      subject: activity.subject,
      from: activity.from,
      verdict: parsed.data.verdict as EmailFeedbackVerdict,
      note: parsed.data.note,
    });
    logger.info(
      `Email feedback: ${feedback.verdict} for "${activity.subject}" (${activity.pipeline})`,
    );
    return c.json({ feedback });
  });

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

  app.get("/api/health", (c) => c.json({ status: "ok" }));

  app.get("/api/pets", async (c) => {
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
  });

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
  app.use("*", async (c, next) => {
    await next();
    if (c.req.path.startsWith("/assets/")) {
      c.res.headers.set("Cache-Control", "public, max-age=31536000, immutable");
    } else if (c.res.headers.get("Content-Type")?.includes("text/html")) {
      c.res.headers.set("Cache-Control", "no-cache");
    }
  });
  app.use("*", serveStatic({ root: "./frontend/dist" }));
  app.use("*", serveStatic({ root: "./frontend/dist", path: "index.html" }));

  const server = serve({ fetch: app.fetch, port }, () => {
    logger.info(`Server listening on port ${port}`);
  });

  return () => {
    unsubscribe();
    clearInterval(heartbeat);
    clearTimeout(debounce);
    server.close();
    // Open SSE streams would otherwise keep the process alive indefinitely.
    if ("closeAllConnections" in server) server.closeAllConnections();
  };
}
