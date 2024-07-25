import cron from "node-cron";
import { debug } from "./logging.js";
import { TaskManager } from "./tasks/taskManager.js";

// Initialize Task Manager
const taskManager = new TaskManager();

cron.schedule(
	"*/20 * * * * *",
	async () => {
		await randomSleep(); // Fuzz
		debug("Running scheduled tasks...");
		await taskManager.runTasks();
	},
	{ runOnInit: true },
);

function randomSleep(maxMilliseconds = 3000): Promise<void> {
	const delay = Math.floor(Math.random() * maxMilliseconds);
	return new Promise((resolve) => setTimeout(resolve, delay));
}
