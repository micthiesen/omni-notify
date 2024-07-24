import type { Config } from "../config.js";

export interface Task {
	name: string;
	run: (config: Config) => Promise<void>;
}
