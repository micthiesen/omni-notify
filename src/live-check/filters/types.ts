import { z } from "zod";
import type { Platform } from "../platforms/index.js";

// LLM structured response schema
export const filterDecisionSchema = z.object({
  shouldNotify: z.boolean(),
  reason: z.string(),
});

export type FilterDecision = z.infer<typeof filterDecisionSchema>;

// Per-channel filter config
export type ChannelFilter = {
  prompt: string;
  defaultOnError: boolean;
};

// Channel config
export type ChannelConfig = {
  pushoverToken?: string;
  filter?: ChannelFilter;
};

// Full channels config file structure
export type ChannelsConfig = {
  [platform: string]: {
    [username: string]: ChannelConfig;
  };
};

// Zod schema for config file validation
const channelFilterSchema = z.object({
  prompt: z.string(),
  defaultOnError: z.boolean(),
});

const channelConfigSchema = z.object({
  pushoverToken: z.string().optional(),
  filter: channelFilterSchema.optional(),
});

export const channelsConfigSchema = z.record(
  z.string(),
  z.record(z.string(), channelConfigSchema),
);

// Stream context passed to filter service
export type StreamContext = {
  username: string;
  displayName: string;
  platform: Platform;
  title: string;
  category?: string;
};

// Filter result returned by service
export type FilterResult = {
  shouldNotify: boolean;
  reason: string;
  /** Whether a filter was actually configured and evaluated */
  wasFiltered: boolean;
};
