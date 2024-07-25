import config, { LogLevel } from "./config.js";
import { sendNotification } from "./notifications.js";

const LOG_LEVEL_MAP: Record<LogLevel, number> = {
	[LogLevel.DEBUG]: 10,
	[LogLevel.INFO]: 20,
	[LogLevel.WARN]: 30,
	[LogLevel.ERROR]: 40,
};
const LOG_LEVEL_NUM = LOG_LEVEL_MAP[config.LOG_LEVEL];

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
function log(level: LogLevel, message: string, ...args: any[]) {
	const levelNum = LOG_LEVEL_MAP[level];
	if (levelNum < LOG_LEVEL_NUM) return;
	console[level](message, ...args);
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
export function debug(message: string, ...args: any[]) {
	log(LogLevel.DEBUG, message, ...args);
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
export function info(message: string, ...args: any[]) {
	log(LogLevel.INFO, message, ...args);
}
// biome-ignore lint/suspicious/noExplicitAny: <explanation>
export function warn(message: string, ...args: any[]) {
	log(LogLevel.WARN, message, ...args);
}

// biome-ignore lint/suspicious/noExplicitAny: <explanation>
export function error(message: string, ...args: any[]) {
	log(LogLevel.ERROR, message, ...args);
	(async () => {
		sendNotification({
			title: `Error: ${message}`,
			message: `${args}`,
		});
	})();
}
