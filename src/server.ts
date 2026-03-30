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

function round(n: number): number {
  return Math.round(n * 100) / 100;
}

export function startServer(port: number, parentLogger: Logger): () => void {
  const logger = parentLogger.extend("Server");
  const app = new Hono();

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
