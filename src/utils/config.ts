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
});

export type Config = z.infer<typeof configSchema>;

const config = configSchema.parse(process.env);
logConfig(config);

export default config;
