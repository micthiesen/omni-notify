/**
 * Dev preview harness: boots the real HTTP server + registry with fake tasks,
 * streamer statuses, runs, recommendations, and a pet so the frontend can be
 * exercised end-to-end without real credentials, APIs, or notifications.
 * Fake tasks take a few seconds to "run" so the realtime flow is observable.
 *
 * Usage: DB_NAME=/tmp/omni-preview.db FRONTEND_PORT=3999 npx tsx src/tools/preview-server.ts
 */
import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { StreamerStatusEntity } from "../live-check/persistence.js";
import { Platform } from "../live-check/platforms/index.js";
import type { Streamer } from "../live-check/streamers.js";
import { insertWeightReading, upsertPet } from "../pet-tracker/persistence.js";
import {
  RecommendationEntity,
  RecommendationStatus,
} from "../recommendations/persistence.js";
import { MediaType } from "../recommendations/types.js";
import { startServer } from "../server.js";
import { TaskRunEntity } from "../task-runs/persistence.js";
import { TaskRegistry } from "../task-runs/registry.js";
import config from "../utils/config.js";

Injector.configure({ config });
const logger = new Logger("Preview");

class FakeTask extends ScheduledTask {
  public constructor(
    public readonly name: string,
    public readonly schedule: string,
    private readonly durationMs: number,
    private readonly summary?: string,
    private readonly fail = false,
  ) {
    super();
  }

  public async run(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, this.durationMs));
    if (this.fail) throw new Error("Simulated failure: upstream returned 503");
  }

  public getLastRunSummary(): string | undefined {
    return this.summary;
  }
}

const now = Date.now();
const MIN = 60_000;
const HOUR = 3_600_000;

// --- Streamers ---------------------------------------------------------------

const streamers: Streamer[] = [
  {
    id: "pixeldust",
    displayName: "PixelDust",
    bindings: [{ platform: Platform.Twitch, username: "pixeldust" }],
  },
  {
    id: "novabyte",
    displayName: "NovaByte",
    bindings: [
      { platform: Platform.YouTube, username: "@novabyte" },
      { platform: Platform.Twitch, username: "novabyte" },
    ],
  },
  {
    id: "retrorex",
    displayName: "RetroRex",
    bindings: [{ platform: Platform.Kick, username: "retrorex" }],
  },
];

StreamerStatusEntity.upsert({
  streamerId: "pixeldust",
  isLive: true,
  primary: { platform: Platform.Twitch, username: "pixeldust" },
  primaryTitle: "Ranked grind to Diamond — day 12, chat picks the loadout",
  startedAt: new Date(now - 2.4 * HOUR),
  maxViewerCount: 4230,
});
StreamerStatusEntity.upsert({
  streamerId: "novabyte",
  isLive: true,
  primary: { platform: Platform.YouTube, username: "@novabyte" },
  primaryTitle: "Building a mechanical keyboard from scratch (live soldering)",
  startedAt: new Date(now - 47 * MIN),
  maxViewerCount: 812,
});
StreamerStatusEntity.upsert({
  streamerId: "retrorex",
  isLive: false,
  lastStartedAt: new Date(now - 29 * HOUR),
  lastEndedAt: new Date(now - 26 * HOUR),
  lastMaxViewerCount: 1890,
});

// --- Task run history ----------------------------------------------------------

let seq = 0;
function seedRun(
  taskName: string,
  startedAt: number,
  durationMs: number,
  status: "success" | "error",
  extra: {
    trigger?: "schedule" | "manual" | "startup";
    error?: string;
    summary?: string;
  } = {},
): void {
  TaskRunEntity.upsert({
    runId: `${taskName}:${startedAt}:${seq++}`,
    taskName,
    trigger: extra.trigger ?? "schedule",
    startedAt,
    finishedAt: startedAt + durationMs,
    status,
    error: extra.error,
    summary: extra.summary,
  });
}

for (let i = 1; i <= 12; i++) {
  seedRun("LiveCheckTask", now - i * 20_000, 700 + i * 13, "success");
}
seedRun("LiveCheckTask", now - 42 * MIN, 1400, "error", {
  error: "Twitch GQL returned 502 for pixeldust",
});
seedRun("MorningBriefing", now - 2 * HOUR, 34_000, "success", {
  summary: "Covered 5 stories: GPU supply, tape-out delays, and more.",
});
seedRun("EveningBriefing", now - 14 * HOUR, 41_000, "error", {
  error: "Tavily search failed after 3 retries: rate limited",
});
seedRun("PetTrackerTask", now - 34 * MIN, 2_300, "success", {
  summary: "Mochi: 11.3 lbs, 3 visits today.",
});
seedRun("Recommendations", now - 22 * HOUR, 96_000, "success", {
  trigger: "manual",
  summary: "Picked The Iron Harvest (2025); added to watchlist.",
});

// --- Recommendations ---------------------------------------------------------

RecommendationEntity.upsert({
  canonicalId: "tmdb:movie:100001",
  tmdbId: 100001,
  mediaType: MediaType.Movie,
  title: "The Iron Harvest",
  year: 2025,
  status: RecommendationStatus.Notified,
  whyForUser:
    "Slow-burn sci-fi with a strong ensemble cast — matches your recent run of cerebral thrillers and clocks in under two hours.",
  caveats: ["Only on physical rental in some regions"],
  confidence: 0.82,
  runDate: "2026-07-14",
  recommendedAt: now - 22 * HOUR,
  notifiedAt: now - 22 * HOUR + MIN,
  watchlistResult: "added",
});
RecommendationEntity.upsert({
  canonicalId: "tmdb:tv:200002",
  tmdbId: 200002,
  mediaType: MediaType.Tv,
  title: "Harbor Lights",
  year: 2024,
  status: RecommendationStatus.Watched,
  whyForUser: "Character-driven mystery, one tight 8-episode season.",
  confidence: 0.74,
  runDate: "2026-06-20",
  recommendedAt: now - 25 * 24 * HOUR,
  notifiedAt: now - 25 * 24 * HOUR + MIN,
  resolvedAt: now - 4 * 24 * HOUR,
  watchlistResult: "added",
});
RecommendationEntity.upsert({
  canonicalId: "tmdb:movie:300003",
  tmdbId: 300003,
  mediaType: MediaType.Movie,
  title: "Static Bloom",
  year: 2023,
  status: RecommendationStatus.Ignored,
  whyForUser: "A24-style character study with a killer soundtrack.",
  caveats: ["Slow first act", "Subtitled"],
  confidence: 0.61,
  runDate: "2026-05-30",
  recommendedAt: now - 46 * 24 * HOUR,
  notifiedAt: now - 46 * 24 * HOUR + MIN,
  resolvedAt: now - 16 * 24 * HOUR,
  watchlistResult: "already_exists",
});

// --- Pet ----------------------------------------------------------------------

upsertPet({
  pet_id: "mochi",
  name: "Mochi",
  current_weight: 11.3,
  updated_at: new Date(now).toISOString(),
});
for (let day = 90; day >= 0; day--) {
  const t = now - day * 24 * HOUR;
  const weight =
    11.6 -
    day * 0.004 +
    Math.sin(day / 6) * 0.15 +
    (((day * 7919) % 13) / 13 - 0.5) * 0.2;
  insertWeightReading({
    pet_id: "mochi",
    timestamp: new Date(t).toISOString(),
    weight: Math.round(weight * 100) / 100,
  });
}

// --- Registry + server ---------------------------------------------------------

const registry = new TaskRegistry(logger);
registry.track(new FakeTask("LiveCheckTask", "*/20 * * * * *", 1_500));
registry.track(
  new FakeTask(
    "MorningBriefing",
    "0 0 9 * * *",
    6_000,
    "Covered 4 stories: chip exports, new GPU rumors, and more.",
  ),
);
registry.track(new FakeTask("EveningBriefing", "0 0 21 * * *", 5_000, undefined, true));
registry.track(new FakeTask("PetTrackerTask", "0 */30 * * * *", 2_500));
registry.track(
  new FakeTask(
    "Recommendations",
    "0 0 17 * * 1,3,5",
    9_000,
    "Picked Harbor Lights S2 (2026); added to watchlist.",
  ),
);

startServer(config.FRONTEND_PORT, logger, registry, streamers);
logger.info("Preview server ready");
