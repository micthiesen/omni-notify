import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import type { Logger } from "@micthiesen/mitools/logging";
import { Hono } from "hono";
import {
  getAllPetsWithHistory,
  getDailyVisitCounts,
  getPet,
  getWeightHistory,
} from "./pet-tracker/persistence.js";
import { getAllRecommendations } from "./recommendations/persistence.js";
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

export function startServer(
  port: number,
  parentLogger: Logger,
  registry: TaskRegistry,
): () => void {
  const logger = parentLogger.extend("Server");
  const app = new Hono();

  app.get("/api/tasks", (c) =>
    c.json({
      tasks: registry.list().map((task) => ({
        ...task,
        lastRun: task.lastRun ? serializeRun(task.lastRun) : null,
      })),
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

  return () => server.close();
}
