import config, { LogLevel } from "./config.js";
import { sendNotification } from "./notifications.js";

const LOG_LEVEL_MAP: Record<LogLevel, number> = {
	[LogLevel.DEBUG]: 10,
	[LogLevel.INFO]: 20,
	[LogLevel.WARN]: 30,
	[LogLevel.ERROR]: 40,
};
const LOG_LEVEL_NUM = LOG_LEVEL_MAP[config.LOG_LEVEL];

export default class Logger {
	public constructor(private name: string) {}

	// biome-ignore lint/suspicious/noExplicitAny: <explanation>
	public log(level: LogLevel, message: string, ...args: any[]) {
		const levelNum = LOG_LEVEL_MAP[level];
		if (levelNum < LOG_LEVEL_NUM) return;
		const messageFinal = `[${level.toUpperCase()}] <${this.name}> ${message}`;
		console[level](messageFinal, ...args);
	}

	// biome-ignore lint/suspicious/noExplicitAny: <explanation>
	public debug(message: string, ...args: any[]) {
		this.log(LogLevel.DEBUG, message, ...args);
	}

	// biome-ignore lint/suspicious/noExplicitAny: <explanation>
	public info(message: string, ...args: any[]) {
		this.log(LogLevel.INFO, message, ...args);
	}
	// biome-ignore lint/suspicious/noExplicitAny: <explanation>
	public warn(message: string, ...args: any[]) {
		this.log(LogLevel.WARN, message, ...args);
	}

	// biome-ignore lint/suspicious/noExplicitAny: <explanation>
	public error(message: string, ...args: any[]) {
		this.log(LogLevel.ERROR, message, ...args);
		(async () => {
			sendNotification({
				title: `Error: ${message}`,
				message: `${args}`,
			});
		})();
	}
}
