import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import type { Logger } from "@micthiesen/mitools/logging";
import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { getStreamerStatus } from "./live-check/persistence.js";
import { platformConfigs } from "./live-check/platforms/index.js";
import type { PlatformBinding, Streamer } from "./live-check/streamers.js";
import {
  getAllPetsWithHistory,
  getDailyVisitCounts,
  getPet,
  getWeightHistory,
} from "./pet-tracker/persistence.js";
import { getAllRecommendations } from "./recommendations/persistence.js";
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

  // Full dashboard state in one payload; also the polling fallback when the
  // SSE stream is unavailable.
  app.get("/api/snapshot", (c) => c.json(buildSnapshot()));

  // Realtime dashboard updates. Pushes a fresh snapshot on connect and
  // whenever any task run starts or finishes (debounced to coalesce bursts).
  app.get("/api/events", (c) =>
    streamSSE(c, async (stream) => {
      let eventId = 0;
      let sending = false;
      let resend = false;
      const send = async (): Promise<void> => {
        if (sending) {
          resend = true;
          return;
        }
        sending = true;
        try {
          await stream.writeSSE({
            event: "snapshot",
            data: JSON.stringify(buildSnapshot()),
            id: String(eventId++),
          });
        } finally {
          sending = false;
          if (resend) {
            resend = false;
            void send();
          }
        }
      };

      let debounce: NodeJS.Timeout | undefined;
      const unsubscribe = taskRunBus.subscribe(() => {
        clearTimeout(debounce);
        debounce = setTimeout(() => void send(), SSE_DEBOUNCE_MS);
      });
      const heartbeat = setInterval(() => {
        void stream.writeSSE({ event: "ping", data: String(Date.now()) });
      }, SSE_HEARTBEAT_MS);

      await send();
      await new Promise<void>((resolve) => {
        stream.onAbort(() => {
          unsubscribe();
          clearInterval(heartbeat);
          clearTimeout(debounce);
          resolve();
        });
      });
    }),
  );

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
    const recommendations = getAllRecommendations().map((rec) => ({
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
      resolvedAt: rec.resolvedAt ?? null,
      watchlistResult: rec.watchlistResult ?? null,
      confidence: rec.confidence ?? null,
    }));
    return c.json({ recommendations });
  });

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

  app.use("*", serveStatic({ root: "./frontend/dist" }));
  app.use("*", serveStatic({ root: "./frontend/dist", path: "index.html" }));

  const server = serve({ fetch: app.fetch, port }, () => {
    logger.info(`Server listening on port ${port}`);
  });

  return () => {
    server.close();
    // Open SSE streams would otherwise keep the process alive indefinitely.
    if ("closeAllConnections" in server) server.closeAllConnections();
  };
}
