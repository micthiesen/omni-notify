import { baseConfigSchema, logConfig, stringBoolean } from "@micthiesen/mitools/config";
import { z } from "zod";

export type ChannelEntry = { username: string; displayName: string };

const channelList = z
  .string()
  .optional()
  .transform((val): ChannelEntry[] => {
    if (!val) return [];
    return val.split(",").map((entry) => {
      const [username, displayName] = entry.split(":");
      return { username, displayName: displayName ?? username };
    });
  });

const configSchema = baseConfigSchema.extend({
  YT_CHANNEL_NAMES: channelList,
  TWITCH_CHANNEL_NAMES: channelList,
  OFFLINE_NOTIFICATIONS: z.string().optional().default("true").transform(stringBoolean),
  GOOGLE_GENERATIVE_AI_API_KEY: z.string().optional(),
  EXA_API_KEY: z.string().optional(),
  CHANNELS_CONFIG_PATH: z.string().optional(),
  SMTP_HOST: z.string().optional(),
  SMTP_PORT: z.coerce.number().optional().default(587),
  SMTP_USER: z.string().optional(),
  SMTP_PASS: z.string().optional(),
  EMAIL_FROM: z.string().optional(),
  LOGS_EMAIL_TO: z.string().optional(),
});

export type Config = z.infer<typeof configSchema>;

const config = configSchema.parse(process.env);
logConfig(config);

export default config;
