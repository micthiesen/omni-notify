import cron from "node-cron";
import { TaskManager } from "./tasks/taskManager.js";

// Initialize Task Manager
const taskManager = new TaskManager();

// Schedule tasks to run every 10 seconds
cron.schedule("*/10 * * * * *", () => {
  console.log("Running scheduled tasks...");
  taskManager.runTasks();
});
