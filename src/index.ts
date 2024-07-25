import cron from "node-cron";
import config from "./config.js";
import { TaskManager } from "./tasks/taskManager.js";

// Initialize Task Manager
const taskManager = new TaskManager(config);

// Schedule tasks to run every 30 seconds
cron.schedule("*/30 * * * * *", () => {
	console.log("Running scheduled tasks...");
	taskManager.runTasks();
});
