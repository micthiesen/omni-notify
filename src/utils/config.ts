import { baseConfigSchema, logConfig } from "@micthiesen/mitools/config";
import { z } from "zod";

const stringBoolean = (value: string): boolean => value.toLowerCase() === "true";
const configSchema = baseConfigSchema.extend({
	YT_CHANNEL_NAMES: z
		.string()
		.optional()
		.transform((val) => (val ? val.split(",") : [])),
	OFFLINE_NOTIFICATIONS: z.string().optional().default("true").transform(stringBoolean),
});

export type Config = z.infer<typeof configSchema>;

const config = configSchema.parse(process.env);
logConfig(config);

export default config;
