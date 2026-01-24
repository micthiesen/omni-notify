import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import cron from "node-cron";
import TaskManager from "./tasks/TaskManager.js";
import config from "./utils/config.js";

Injector.configure({ config });

const logger = new Logger("Main");
const taskManager = new TaskManager(logger);

const cronTask = cron.schedule("*/20 * * * * *", async () => {
  await randomSleep();
  logger.debug("Running scheduled tasks...");
  await taskManager.runTasks();
});

cronTask.execute();

// Graceful shutdown handling
let isShuttingDown = false;

async function shutdown(signal: string): Promise<void> {
  if (isShuttingDown) return;
  isShuttingDown = true;

  logger.info(`Received ${signal}, shutting down gracefully...`);
  cronTask.stop();

  await taskManager.waitForPending();
  logger.info("Shutdown complete");
  process.exit(0);
}

process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));

function randomSleep(maxMilliseconds = 3000): Promise<void> {
  const delay = Math.floor(Math.random() * maxMilliseconds);
  return new Promise((resolve) => setTimeout(resolve, delay));
}
