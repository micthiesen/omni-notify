import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Platform } from "../platforms/index.js";
import {
  type ChannelConfig,
  type ChannelFilter,
  type ChannelsConfig,
  channelsConfigSchema,
} from "./types.js";

const DEFAULT_CONFIG_PATH = "./channels.json";
const CACHE_TTL_MS = 60_000; // 60 seconds

let cachedConfig: ChannelsConfig | null = null;
let cacheTimestamp = 0;

function getConfigPath(): string {
  return process.env.CHANNELS_CONFIG_PATH || DEFAULT_CONFIG_PATH;
}

export function loadChannelsConfig(logger?: Logger): ChannelsConfig {
  const now = Date.now();

  // Return cached config if still valid
  if (cachedConfig && now - cacheTimestamp < CACHE_TTL_MS) {
    return cachedConfig;
  }

  const configPath = resolve(getConfigPath());

  try {
    const content = readFileSync(configPath, "utf-8");
    const parsed = JSON.parse(content);
    const validated = channelsConfigSchema.parse(parsed);

    cachedConfig = validated;
    cacheTimestamp = now;
    return validated;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      // File doesn't exist - this is fine, no filters applied
      cachedConfig = {};
      cacheTimestamp = now;
      return {};
    }

    // Invalid JSON or schema validation error
    logger?.warn(`Failed to load channels config: ${error}`);
    cachedConfig = {};
    cacheTimestamp = now;
    return {};
  }
}

export function getChannelConfig(
  platform: Platform,
  username: string,
  logger?: Logger,
): ChannelConfig | null {
  const config = loadChannelsConfig(logger);
  return config[platform]?.[username] ?? null;
}

export function getChannelFilter(
  platform: Platform,
  username: string,
  logger?: Logger,
): ChannelFilter | null {
  const channelConfig = getChannelConfig(platform, username, logger);
  return channelConfig?.filter ?? null;
}

// For testing: clear the cache
export function clearConfigCache(): void {
  cachedConfig = null;
  cacheTimestamp = 0;
}
