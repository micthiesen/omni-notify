import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import { BriefingAgentTask } from "./briefing-agent/BriefingAgentTask.js";
import { loadBriefingConfigs } from "./briefing-agent/configs.js";
import { createCalendarHandler } from "./calendar-events/index.js";
import { createJmapClient } from "./jmap/client.js";
import { EmailDispatcher } from "./jmap/dispatcher.js";
import { createEventSource } from "./jmap/eventSource.js";
import { loadChannelsConfig } from "./live-check/filters/index.js";
import { Platform } from "./live-check/platforms/index.js";
import LiveCheckTask from "./live-check/task.js";
import { createParcelHandler } from "./parcel-tracker/index.js";
import { Scheduler } from "./scheduling/Scheduler.js";
import config from "./utils/config.js";

Injector.configure({ config });

const logger = new Logger("Main");
const scheduler = new Scheduler(logger);

// Register tasks
const channels: [Platform, { username: string; displayName: string }[]][] = [
  [Platform.YouTube, config.YT_CHANNEL_NAMES],
  [Platform.Twitch, config.TWITCH_CHANNEL_NAMES],
];
const channelsConfig = loadChannelsConfig(logger);
scheduler.register(new LiveCheckTask(channels, channelsConfig, logger));

for (const config of loadBriefingConfigs(logger)) {
  const task = BriefingAgentTask.create(config, logger);
  if (task) scheduler.register(task);
}

// Start JMAP-based features (parcel tracker + calendar events)
let cleanupEventSource: (() => void) | undefined;
try {
  cleanupEventSource = await startJmapFeatures(logger);
} catch (error) {
  logger.error("Failed to start JMAP features", (error as Error).message);
}

// Start scheduler (runs tasks immediately, then on their schedules)
scheduler.start();

// Graceful shutdown handling
let isShuttingDown = false;

async function shutdown(signal: string): Promise<void> {
  if (isShuttingDown) return;
  isShuttingDown = true;

  logger.info(`Received ${signal}, shutting down gracefully...`);
  cleanupEventSource?.();
  await scheduler.shutdown();
  logger.info("Shutdown complete");
  process.exit(0);
}

process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));

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
