import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import { loadChannelsConfig } from "./live-check/filters/index.js";
import { Platform } from "./live-check/platforms/index.js";
import LiveCheckTask from "./live-check/task.js";
import NewsAgentTask from "./news-agent/task.js";
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

const newsAgent = NewsAgentTask.create(logger);
if (newsAgent) scheduler.register(newsAgent);

// Start scheduler (runs tasks immediately, then on their schedules)
scheduler.start();

// Graceful shutdown handling
let isShuttingDown = false;

async function shutdown(signal: string): Promise<void> {
  if (isShuttingDown) return;
  isShuttingDown = true;

  logger.info(`Received ${signal}, shutting down gracefully...`);
  await scheduler.shutdown();
  logger.info("Shutdown complete");
  process.exit(0);
}

process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));
