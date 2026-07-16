import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Scheduler } from "@micthiesen/mitools/scheduling";
import { BriefingAgentTask } from "./briefing-agent/BriefingAgentTask.js";
import { loadBriefingConfigs } from "./briefing-agent/configs.js";
import { createCalendarHandler } from "./calendar-events/index.js";
import { createJmapClient } from "./jmap/client.js";
import { EmailDispatcher } from "./jmap/dispatcher.js";
import { createEventSource } from "./jmap/eventSource.js";
import { loadChannelsConfig } from "./live-check/channelsConfig.js";
import { Platform } from "./live-check/platforms/index.js";
import { buildStreamers, type Streamer } from "./live-check/streamers.js";
import LiveCheckTask from "./live-check/task.js";
import { createParcelHandler } from "./parcel-tracker/index.js";
import PetTrackerTask from "./pet-tracker/task.js";
import { CastroQueueCleanupTask } from "./podcast-recs/castro/queueCleanupTask.js";
import { PodcastRecommendationTask } from "./podcast-recs/task.js";
import { migrateLegacyRecommendations } from "./recommendations/persistence.js";
import { RecommendationTask } from "./recommendations/task.js";
import { TasteReflectionTask } from "./recommendations/taste/task.js";
import { startServer } from "./server.js";
import { installLogCapture } from "./task-runs/logCapture.js";
import { TaskRegistry } from "./task-runs/registry.js";
import config from "./utils/config.js";

Injector.configure({ config });
installLogCapture();
migrateLegacyRecommendations();

const logger = new Logger("Main");

function loadStreamers(): Streamer[] {
  const kickConfigured = config.KICK_CLIENT_ID && config.KICK_CLIENT_SECRET;
  if (config.KICK_CHANNEL_NAMES.length > 0 && !kickConfigured) {
    logger.warn(
      "Kick channels configured but KICK_CLIENT_ID/KICK_CLIENT_SECRET missing; skipping Kick",
    );
  }
  const sources: [Platform, { username: string; displayName: string }[]][] = [
    [Platform.YouTube, config.YT_CHANNEL_NAMES],
    [Platform.Twitch, config.TWITCH_CHANNEL_NAMES],
    [Platform.Kick, kickConfigured ? config.KICK_CHANNEL_NAMES : []],
  ];
  return buildStreamers(sources, loadChannelsConfig(logger));
}

function buildTasks(streamers: Streamer[]): ScheduledTask[] {
  const tasks: ScheduledTask[] = [];

  if (streamers.length > 0) {
    tasks.push(new LiveCheckTask(streamers, logger));
  }
  if (config.WHISKER_CREDENTIALS) {
    tasks.push(new PetTrackerTask(config.WHISKER_CREDENTIALS, logger));
  }

  for (const config of loadBriefingConfigs(logger)) {
    const task = BriefingAgentTask.create(config, logger);
    if (task) tasks.push(task);
  }

  const recommendations = RecommendationTask.create(logger);
  if (recommendations) tasks.push(recommendations);
  const podcastRecs = PodcastRecommendationTask.create(logger);
  if (podcastRecs) tasks.push(podcastRecs);
  const castroQueueCleanup = CastroQueueCleanupTask.create(logger);
  if (castroQueueCleanup) tasks.push(castroQueueCleanup);
  const tasteReflection = TasteReflectionTask.create(logger);
  if (tasteReflection) tasks.push(tasteReflection);

  return tasks;
}

// --run-task <name>: run a single task once and exit
const runTaskIndex = process.argv.indexOf("--run-task");
if (runTaskIndex !== -1) {
  const taskName = process.argv[runTaskIndex + 1];
  if (!taskName) {
    logger.error("Usage: --run-task <TaskName>");
    process.exit(1);
  }

  const tasks = buildTasks(loadStreamers());
  const task = tasks.find((t) => t.name.toLowerCase() === taskName.toLowerCase());
  if (!task) {
    const names = tasks.map((t) => t.name).join(", ");
    logger.error(`Unknown task "${taskName}". Available: ${names}`);
    process.exit(1);
  }

  logger.info(`Running task "${task.name}" once...`);
  await task.run();
  logger.info(`Task "${task.name}" complete`);
  process.exit(0);
}

// --server-only: just the HTTP server, no tasks
const serverOnly = process.argv.includes("--server-only");

const registry = new TaskRegistry(logger);
const streamers = loadStreamers();

// Start HTTP server
const closeServer = startServer(config.FRONTEND_PORT, logger, registry, streamers);

let cleanupEventSource: (() => void) | undefined;

if (!serverOnly) {
  const scheduler = new Scheduler(logger);
  for (const task of buildTasks(streamers)) {
    scheduler.register(registry.track(task));
  }

  // Start JMAP-based features (parcel tracker + calendar events)
  try {
    cleanupEventSource = await startJmapFeatures(logger);
  } catch (error) {
    logger.error("Failed to start JMAP features", (error as Error).message);
  }

  // Start scheduler (runs opted-in tasks immediately, then all tasks on schedule)
  scheduler.start();
  const recoveryPromise = registry.recoverMissedTasks().catch((error) => {
    logger.error("Failed to recover missed task runs", error);
  });

  // Graceful shutdown handling
  let isShuttingDown = false;

  async function shutdown(signal: string): Promise<void> {
    if (isShuttingDown) return;
    isShuttingDown = true;

    logger.info(`Received ${signal}, shutting down gracefully...`);
    closeServer();
    cleanupEventSource?.();
    await scheduler.shutdown();
    await recoveryPromise;
    logger.info("Shutdown complete");
    process.exit(0);
  }

  process.on("SIGTERM", () => shutdown("SIGTERM"));
  process.on("SIGINT", () => shutdown("SIGINT"));
} else {
  logger.info("Running in server-only mode (tasks disabled)");
}

async function startJmapFeatures(
  parentLogger: Logger,
): Promise<(() => void) | undefined> {
  if (!config.FASTMAIL_API_TOKEN) {
    parentLogger.info("JMAP features disabled: missing FASTMAIL_API_TOKEN");
    return undefined;
  }

  const jmapLogger = parentLogger.extend("JMAP");
  const ctx = await createJmapClient(config.FASTMAIL_API_TOKEN, jmapLogger);

  // Create dispatcher and register handlers
  const dispatcher = new EmailDispatcher(ctx, jmapLogger);

  const parcel = createParcelHandler(parentLogger);
  if (parcel) dispatcher.register(parcel);

  const calendar = createCalendarHandler(ctx, parentLogger);
  if (calendar) dispatcher.register(calendar);

  if (dispatcher.handlerCount === 0) {
    jmapLogger.info("No JMAP pipelines active");
    return undefined;
  }

  const closeEventSource = await createEventSource(
    ctx,
    () => dispatcher.onStateChange(),
    jmapLogger,
  );
  jmapLogger.info(`Started with ${dispatcher.handlerCount} pipeline(s)`);
  return closeEventSource;
}
