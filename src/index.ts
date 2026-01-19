import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import cron from "node-cron";
import TaskManager from "./tasks/TaskManager.js";
import config from "./utils/config.js";

Injector.configure({ config });

const logger = new Logger("Main");
const taskManager = new TaskManager(logger);

cron.schedule(
  "*/20 * * * * *",
  async () => {
    await randomSleep(); // Fuzz
    logger.debug("Running scheduled tasks...");
    await taskManager.runTasks();
  },
  { runOnInit: true },
);

function randomSleep(maxMilliseconds = 3000): Promise<void> {
  const delay = Math.floor(Math.random() * maxMilliseconds);
  return new Promise((resolve) => setTimeout(resolve, delay));
}
