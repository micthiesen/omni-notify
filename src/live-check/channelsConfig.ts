import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { z } from "zod";

const DEFAULT_CONFIG_PATH = "./channels.json";

// A username/handle/slug on one platform, or several (e.g. two Twitch
// accounts for the same person). Empty strings and empty arrays are rejected
// explicitly so a typo'd trailing comma in a hand-edited file fails loudly.
const usernamesSchema = z.union([
  z.string().min(1, "must not be empty"),
  z.array(z.string().min(1, "must not be empty")).min(1, "must not be empty"),
]);

// Strict: this file is hand-edited, so an unknown key (a typo'd field name)
// should fail validation rather than silently do nothing.
const channelEntrySchema = z
  .object({
    youtube: usernamesSchema.optional(),
    twitch: usernamesSchema.optional(),
    kick: usernamesSchema.optional(),
    pushoverToken: z.string().optional(),
    liveNotifications: z.boolean().optional(),
    tier: z.enum(["primary", "background"]).optional(),
  })
  .strict()
  .superRefine((value, ctx) => {
    if (
      value.youtube === undefined &&
      value.twitch === undefined &&
      value.kick === undefined
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: "must specify at least one platform (youtube, twitch, or kick)",
      });
    }
    // The background tier already implies liveNotifications: false, so an
    // explicit liveNotifications is always either contradictory or redundant —
    // both are almost certainly a config mistake worth failing loudly on.
    if (value.tier === "background" && value.liveNotifications !== undefined) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["liveNotifications"],
        message:
          value.liveNotifications === true
            ? "liveNotifications: true contradicts the background tier, which mutes live notifications."
            : "liveNotifications: false is redundant, the background tier already mutes live notifications.",
      });
    }
  });

// Keys are display names; a blank key would produce a nameless streamer with
// id "", so it fails loudly like every other typo in this hand-edited file.
export const channelsConfigSchema = z.record(
  z.string().refine((s) => s.trim().length > 0, "display name must not be blank"),
  channelEntrySchema,
);
export type ChannelEntry = z.infer<typeof channelEntrySchema>;
export type ChannelsConfig = Record<string, ChannelEntry>;

/**
 * Loads channels.json — the single source of truth for which streamers to
 * track, their platform usernames, and their per-streamer overrides. Keyed by
 * display name; see streamers.ts:buildStreamers for how entries become
 * Streamer[] (including cross-entry checks like duplicate display names and
 * duplicate platform bindings, which need the whole map at once and so aren't
 * done here).
 *
 * A missing file is a valid "no streamers configured" state (ENOENT → {});
 * any other failure — bad JSON, a schema violation, a superRefine failure —
 * throws instead of warning and falling back to {}. Failing open here would
 * silently drop streamers or un-mute ones that relied on overrides, so a
 * broken config must fail boot rather than fail open.
 */
export function loadChannelsConfig(): ChannelsConfig {
  const configPath = resolve(process.env.CHANNELS_CONFIG_PATH || DEFAULT_CONFIG_PATH);
  let content: string;
  try {
    content = readFileSync(configPath, "utf-8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw new Error(`Failed to read channels config at "${configPath}": ${error}`);
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch (error) {
    throw new Error(`Failed to parse channels config at "${configPath}": ${error}`);
  }

  const result = channelsConfigSchema.safeParse(parsed);
  if (!result.success) {
    throw new Error(
      `Invalid channels config at "${configPath}": ${result.error.message}`,
    );
  }
  return result.data;
}
