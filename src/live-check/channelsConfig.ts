import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Logger } from "@micthiesen/mitools/logging";
import { z } from "zod";
import type { StreamerOverride } from "./streamers.js";

const DEFAULT_CONFIG_PATH = "./channels.json";

const streamerOverrideSchema = z.object({
  pushoverToken: z.string().optional(),
});

export const channelsConfigSchema = z.record(z.string(), streamerOverrideSchema);
export type ChannelsConfig = Record<string, StreamerOverride>;

export function loadChannelsConfig(logger: Logger): ChannelsConfig {
  const configPath = resolve(process.env.CHANNELS_CONFIG_PATH || DEFAULT_CONFIG_PATH);
  try {
    const content = readFileSync(configPath, "utf-8");
    const parsed = JSON.parse(content);
    return channelsConfigSchema.parse(parsed);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    logger.warn(`Failed to load channels config: ${error}`);
    return {};
  }
}
