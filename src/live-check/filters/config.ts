import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Logger } from "@micthiesen/mitools/logging";
import { type ChannelsConfig, channelsConfigSchema } from "./types.js";

const DEFAULT_CONFIG_PATH = "./channels.json";

function getConfigPath(): string {
  return process.env.CHANNELS_CONFIG_PATH || DEFAULT_CONFIG_PATH;
}

export function loadChannelsConfig(logger: Logger): ChannelsConfig {
  const configPath = resolve(getConfigPath());

  try {
    const content = readFileSync(configPath, "utf-8");
    const parsed = JSON.parse(content);
    return channelsConfigSchema.parse(parsed);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      // File doesn't exist - this is fine, no channel-specific config
      return {};
    }

    // Invalid JSON or schema validation error
    logger.warn(`Failed to load channels config: ${error}`);
    return {};
  }
}
