import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import type { Logger } from "@micthiesen/mitools/logging";
import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { z } from "zod";
import { getViewerMetrics } from "./live-check/metrics/persistence.js";
import { getStreamerStatus } from "./live-check/persistence.js";
import { platformConfigs } from "./live-check/platforms/index.js";
import type { PlatformBinding, Streamer } from "./live-check/streamers.js";
import {
  getAllPetsWithHistory,
  getDailyVisitCounts,
  getPet,
  getWeightHistory,
} from "./pet-tracker/persistence.js";
import {
  getAllRecommendations,
  getRecommendation,
  type RecommendationData,
  setRecommendationFeedback,
} from "./recommendations/persistence.js";
import { getLatestTasteProfile } from "./recommendations/taste/index.js";
import { taskRunBus } from "./task-runs/events.js";
import { getRuns, type TaskRunData } from "./task-runs/persistence.js";
import {
  TaskAlreadyRunningError,
  TaskNotFoundError,
  type TaskRegistry,
} from "./task-runs/registry.js";

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
    startedAt: run.startedAt,
    finishedAt: run.finishedAt ?? null,
    status: run.status,
    error: run.error ?? null,
    summary: run.summary ?? null,
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
      plex: "http://plex.boris/web/index.html#!/",
      manager:
        rec.mediaType === "movie" ? "http://radarr.boris/" : "http://sonarr.boris/",
    },
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

export function startServer(
  port: number,
  parentLogger: Logger,
  registry: TaskRegistry,
  streamers: Streamer[],
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

  app.post("/api/tasks/:name/run", (c) => {
    const name = c.req.param("name");
    try {
      const { runId } = registry.runNow(name);
      logger.info(`Manual run requested for "${name}"`);
      return c.json({ runId }, 202);
    } catch (error) {
      if (error instanceof TaskNotFoundError) {
        return c.json({ error: error.message }, 404);
      }
      if (error instanceof TaskAlreadyRunningError) {
        return c.json({ error: error.message }, 409);
      }
      throw error;
    }
  });

  app.get("/api/task-runs", (c) => {
    const task = c.req.query("task");
    const limitParam = Number(c.req.query("limit"));
    const limit =
      Number.isInteger(limitParam) && limitParam > 0 ? Math.min(limitParam, 200) : 50;
    return c.json({ runs: getRuns(task || undefined, limit).map(serializeRun) });
  });

  app.get("/api/recommendations", (c) => {
    const recommendations = getAllRecommendations().map(serializeRecommendation);
    return c.json({ recommendations });
  });

  app.get("/api/recommendations/taste-profile", (c) =>
    c.json({ profile: getLatestTasteProfile() ?? null }),
  );

  const feedbackSchema = z.object({
    feedback: z.enum(["good_pick", "not_for_me", "already_watched"]),
  });

  app.post("/api/recommendations/:id/feedback", async (c) => {
    const parsed = feedbackSchema.safeParse(await c.req.json().catch(() => null));
    if (!parsed.success) {
      return c.json({ error: "Invalid recommendation feedback" }, 400);
    }
    const existing = getRecommendation(c.req.param("id"));
    if (!existing) return c.json({ error: "Recommendation not found" }, 404);
    if (existing.status === "pending" || existing.status === "failed") {
      return c.json({ error: "Undelivered recommendations cannot be rated" }, 409);
    }
    const recommendation = setRecommendationFeedback(
      c.req.param("id"),
      parsed.data.feedback,
    );
    if (!recommendation) return c.json({ error: "Recommendation not found" }, 404);
    return c.json({ recommendation: serializeRecommendation(recommendation) });
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
