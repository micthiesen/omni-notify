import { baseConfigSchema } from "@micthiesen/mitools";
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
console.log(
	"Config:",
	Object.fromEntries(
		Object.entries(config).map(([key, value]) => [
			key,
			key.includes("TOKEN") || key.includes("KEY") || key.includes("USER")
				? "***"
				: value,
		]),
	),
);

export default config;
