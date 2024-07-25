import cron from "node-cron";
import config from "./config.js";
import { TaskManager } from "./tasks/taskManager.js";

// Initialize Task Manager
const taskManager = new TaskManager(config);

cron.schedule("*/20 * * * * *", () => {
	// console.log("Running scheduled tasks...");
	taskManager.runTasks();
});
