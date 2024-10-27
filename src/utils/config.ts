import { baseConfigSchema, logConfig, stringBoolean } from "@micthiesen/mitools/config";
import { z } from "zod";

const commaSeparatedString = z
	.string()
	.optional()
	.transform((val) => (val ? val.split(",") : []));

(value: string): string[] => value.split(",");
const configSchema = baseConfigSchema.extend({
	YT_CHANNEL_NAMES: commaSeparatedString,
	KICK_CHANNEL_NAMES: commaSeparatedString,
	OFFLINE_NOTIFICATIONS: z.string().optional().default("true").transform(stringBoolean),
});

export type Config = z.infer<typeof configSchema>;

const config = configSchema.parse(process.env);
logConfig(config);

export default config;
