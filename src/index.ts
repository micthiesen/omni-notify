import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Scheduler } from "@micthiesen/mitools/scheduling";
import { BriefingAgentTask } from "./briefing-agent/BriefingAgentTask.js";
import { loadBriefingConfigs } from "./briefing-agent/configs.js";
import { createCalendarHandler } from "./calendar-events/index.js";
import { createJmapClient, type JmapContext } from "./jmap/client.js";
import { EmailDispatcher, type EmailHandler } from "./jmap/dispatcher.js";
import { createEventSource } from "./jmap/eventSource.js";
import EmailRetryTask from "./jmap/retryTask.js";
import { EmailTriageService } from "./jmap/triage.js";
import EmailWatchdogTask from "./jmap/watchdogTask.js";
import { loadChannelsConfig } from "./live-check/channelsConfig.js";
import { Platform } from "./live-check/platforms/index.js";
import { buildStreamers, type Streamer } from "./live-check/streamers.js";
import LiveCheckTask from "./live-check/task.js";
import { createParcelHandler } from "./parcel-tracker/index.js";
import PetTrackerTask from "./pet-tracker/task.js";
import { CastroInboxCleanupTask } from "./podcast-recs/castro/inboxCleanupTask.js";
import { PodcastTasteReflectionTask } from "./podcast-recs/reflection/index.js";
import { PodcastRecommendationTask } from "./podcast-recs/task.js";
import PressPodsTask from "./press-pods/task.js";
import { MediaRecommendationTask } from "./recommendations/task.js";
import { MediaTasteReflectionTask } from "./recommendations/taste/task.js";
import { type EmailControls, startServer } from "./server.js";
import { installLogCapture } from "./task-runs/logCapture.js";
import { TaskRegistry } from "./task-runs/registry.js";
import config from "./utils/config.js";

Injector.configure({ config });
installLogCapture();

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

  const pressPods = PressPodsTask.create(logger);
  if (pressPods) tasks.push(pressPods);
  const recommendations = MediaRecommendationTask.create(logger);
  if (recommendations) tasks.push(recommendations);
  const podcastRecs = PodcastRecommendationTask.create(logger);
  if (podcastRecs) tasks.push(podcastRecs);
  const castroInboxCleanup = CastroInboxCleanupTask.create(logger);
  if (castroInboxCleanup) tasks.push(castroInboxCleanup);
  const tasteReflection = MediaTasteReflectionTask.create(logger);
  if (tasteReflection) tasks.push(tasteReflection);
  const podcastTasteReflection = PodcastTasteReflectionTask.create(logger);
  if (podcastTasteReflection) tasks.push(podcastTasteReflection);

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

// Filled in once the JMAP features start; powers the reprocess endpoint.
const emailControls: EmailControls = {};

// Start HTTP server
const closeServer = startServer(
  config.FRONTEND_PORT,
  logger,
  registry,
  streamers,
  emailControls,
);

let cleanupEventSource: (() => void) | undefined;

if (!serverOnly) {
  const scheduler = new Scheduler(logger);
  for (const task of buildTasks(streamers)) {
    scheduler.register(registry.track(task));
  }

  // Email tasks register up-front (Scheduler requires pre-start registration)
  // so a failed JMAP connect at boot can't silently disable them — the exact
  // outage the watchdog exists to catch. The retry task no-ops until the
  // controls fill in; the connect itself retries in the background.
  if (config.FASTMAIL_API_TOKEN) {
    scheduler.register(registry.track(new EmailWatchdogTask(logger)));
    scheduler.register(registry.track(new EmailRetryTask(() => emailControls, logger)));
    void startJmapWithRetry(logger);
  } else {
    logger.info("JMAP features disabled: missing FASTMAIL_API_TOKEN");
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

interface JmapFeatures {
  cleanup: () => void;
  ctx: JmapContext;
  handlers: Map<string, EmailHandler>;
}

/**
 * Containers restart often and Fastmail can blip: a one-shot connect at boot
 * would silently disable the whole email system (including the retry drain)
 * until the next restart. Retry forever with capped backoff instead; only the
 * first failure alerts (errors reach Pushover), and the watchdog covers the
 * prolonged-outage case.
 */
async function startJmapWithRetry(parentLogger: Logger): Promise<void> {
  const maxDelayMs = 5 * 60_000;
  for (let attempt = 1; ; attempt++) {
    try {
      const jmap = await startJmapFeatures(parentLogger);
      if (jmap) {
        cleanupEventSource = jmap.cleanup;
        emailControls.ctx = jmap.ctx;
        emailControls.handlers = jmap.handlers;
      }
      return;
    } catch (error) {
      const delayMs = Math.min(30_000 * 2 ** (attempt - 1), maxDelayMs);
      const message = `Failed to start JMAP features (attempt ${attempt}), retrying in ${Math.round(delayMs / 1000)}s`;
      if (attempt === 1) {
        parentLogger.error(message, (error as Error).message);
      } else {
        parentLogger.info(`${message}: ${(error as Error).message}`);
      }
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }
}

async function startJmapFeatures(
  parentLogger: Logger,
): Promise<JmapFeatures | undefined> {
  if (!config.FASTMAIL_API_TOKEN) return undefined;

  const jmapLogger = parentLogger.extend("JMAP");
  const ctx = await createJmapClient(config.FASTMAIL_API_TOKEN, jmapLogger);

  // Create dispatcher and register handlers; one shared triage service so
  // concurrent pipelines classify each email with a single model call.
  const dispatcher = new EmailDispatcher(ctx, jmapLogger);
  const triage = new EmailTriageService(jmapLogger.extend("Triage"));

  const parcel = createParcelHandler(parentLogger, triage);
  if (parcel) dispatcher.register(parcel);

  const calendar = createCalendarHandler(ctx, parentLogger, triage);
  if (calendar) dispatcher.register(calendar);

  if (dispatcher.handlerCount === 0) {
    jmapLogger.info("No JMAP pipelines active");
    return undefined;
  }

  const handlers = new Map<string, EmailHandler>();
  if (parcel) handlers.set(parcel.name, parcel);
  if (calendar) handlers.set(calendar.name, calendar);

  const closeEventSource = await createEventSource(
    ctx,
    () => dispatcher.onStateChange(),
    jmapLogger,
  );
  jmapLogger.info(`Started with ${dispatcher.handlerCount} pipeline(s)`);
  return { cleanup: closeEventSource, ctx, handlers };
}
