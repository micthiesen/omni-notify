import { baseConfigSchema } from "@micthiesen/mitools";
import dotenv from "dotenv";
import { z } from "zod";

// Load environment variables from .env file if it exists
dotenv.config();

// Define a Zod schema for the environment variables
const stringBoolean = (value: string): boolean => value.toLowerCase() === "true";
const envSchema = baseConfigSchema.extend({
	YT_CHANNEL_NAMES: z
		.string()
		.optional()
		.transform((val) => (val ? val.split(",") : [])),
	OFFLINE_NOTIFICATIONS: z.string().optional().default("true").transform(stringBoolean),
});

export type Config = z.infer<typeof envSchema>;

const config = envSchema.parse(process.env);
console.log(
	"Config:",
	Object.fromEntries(
		Object.entries(config).map(([key, value]) => [
			key,
			key.includes("TOKEN") || key.includes("KEY") ? "***" : value,
		]),
	),
);

export default config;
