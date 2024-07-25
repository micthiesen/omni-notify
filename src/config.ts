import dotenv from "dotenv";
import { z } from "zod";

// Load environment variables from .env file if it exists
dotenv.config();

// Define a Zod schema for the environment variables
const envSchema = z.object({
	YT_CHANNEL_NAMES: z
		.string()
		.optional()
		.transform((val) => (val ? val.split(",") : [])),
	PUSHOVER_USER_KEY: z.string(),
	PUSHOVER_APP_TOKEN: z.string(),
});

export type Config = z.infer<typeof envSchema>;

const config = envSchema.parse(process.env);
console.log(
	"Loaded config",
	Object.fromEntries(
		Object.entries(config).map(([key, value]) => [
			key,
			key.includes("TOKEN") || key.includes("KEY") ? "***" : value,
		]),
	),
);

export default config;