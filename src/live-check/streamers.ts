import type { ChannelsConfig } from "./channelsConfig.js";
import { Platform } from "./platforms/index.js";

// Tiebreak order when multiple bindings go live in the same tick.
// Earlier = higher priority. Once a primary is elected, it sticks until it goes
// offline; this list only matters for fresh go-live ticks or re-election.
export const PLATFORM_PRIORITY: readonly Platform[] = [
  Platform.YouTube,
  Platform.Twitch,
  Platform.Kick,
];

export type PlatformBinding = {
  platform: Platform;
  username: string;
  /** Exact media URL for transient discovery sources such as YouTube videos. */
  urlOverride?: string;
};

export type StreamerTier = "primary" | "background";

export type DggPresence = {
  hosted: boolean;
  /** Number of Destiny.gg viewers showing this embed, when applicable. */
  viewers: number | null;
};

export type Streamer = {
  id: string;
  displayName: string;
  bindings: PlatformBinding[];
  pushoverToken?: string;
  /**
   * When false, suppresses live/offline/title-change notifications for this
   * streamer. Viewer-record notifications and all tracking still happen.
   */
  liveNotifications?: boolean;
  /**
   * "background" mutes live-activity notifications (same effect as
   * liveNotifications: false), restricts viewer-record notifications to
   * all-time records only, and slows polling in LiveCheckTask. Defaults to
   * "primary"; see notificationPolicy.ts for the exact semantics.
   */
  tier: StreamerTier;
  /** Current Destiny.gg placement metadata, discovered or explicitly configured. */
  dgg?: DggPresence;
};

export function normalizeId(displayName: string): string {
  return displayName.trim().toLowerCase();
}

/**
 * Background streamers are polled every Nth LiveCheckTask tick (20s base →
 * 60s effective); primary streamers every tick.
 */
export const BACKGROUND_POLL_FACTOR = 3;

/**
 * Whether a streamer should be polled on this tick. Tick 0 (the startup run)
 * always polls everyone, including background streamers.
 */
export function isStreamerDue(tier: StreamerTier, tick: number): boolean {
  return tier !== "background" || tick % BACKGROUND_POLL_FACTOR === 0;
}

// channels.json field name → platform. The object literal's key order below
// is otherwise arbitrary: it only sets the order bindings land in
// Streamer.bindings, which nothing depends on (comparePlatformPriority is
// re-derived from PLATFORM_PRIORITY wherever priority actually matters).
const PLATFORM_FIELDS: Record<"youtube" | "twitch" | "kick", Platform> = {
  youtube: Platform.YouTube,
  twitch: Platform.Twitch,
  kick: Platform.Kick,
};

function toUsernames(value: string | string[] | undefined): string[] {
  if (value === undefined) return [];
  return Array.isArray(value) ? value : [value];
}

/**
 * Builds Streamer[] directly from the parsed channels.json config: one entry
 * per streamer, keyed by display name, with platform usernames and overrides
 * both living on the same entry (see channelsConfig.ts for the per-entry
 * shape). Throws on a duplicate normalized display name (two entries
 * collapsing to the same id) or a platform binding reused across
 * entries/arrays (e.g. the same Twitch username listed twice).
 */
export function buildStreamers(config: ChannelsConfig): Streamer[] {
  const byId = new Map<string, Streamer>();
  const seenBindings = new Set<string>();

  for (const [displayName, entry] of Object.entries(config)) {
    const id = normalizeId(displayName);
    if (byId.has(id)) {
      throw new Error(
        `Duplicate streamer "${displayName}" in channels config (normalizes to ` +
          `"${id}", already used by another entry)`,
      );
    }

    const bindings: PlatformBinding[] = [];
    const fields = Object.keys(PLATFORM_FIELDS) as (keyof typeof PLATFORM_FIELDS)[];
    for (const field of fields) {
      const platform = PLATFORM_FIELDS[field];
      for (const username of toUsernames(entry[field])) {
        const bindingKey = `${platform}:${username}`;
        if (seenBindings.has(bindingKey)) {
          throw new Error(
            `Duplicate platform binding "${bindingKey}" across channel entries`,
          );
        }
        seenBindings.add(bindingKey);
        bindings.push({ platform, username });
      }
    }

    byId.set(id, {
      id,
      displayName,
      bindings,
      tier: entry.tier ?? "primary",
      pushoverToken: entry.pushoverToken,
      liveNotifications: entry.liveNotifications,
    });
  }

  return [...byId.values()];
}

/**
 * Drops all bindings for a platform that turned out not to be usable (e.g.
 * Kick configured in channels.json but KICK_CLIENT_ID/SECRET missing at
 * boot), removing any streamer left with zero bindings entirely. Returns
 * whether anything was actually dropped, so the caller can decide whether to
 * warn.
 */
export function dropPlatformBindings(
  streamers: Streamer[],
  platform: Platform,
): { streamers: Streamer[]; droppedAny: boolean } {
  let droppedAny = false;
  const result = streamers
    .map((s) => {
      if (!s.bindings.some((b) => b.platform === platform)) return s;
      droppedAny = true;
      return { ...s, bindings: s.bindings.filter((b) => b.platform !== platform) };
    })
    .filter((s) => s.bindings.length > 0);
  return { streamers: result, droppedAny };
}

export function comparePlatformPriority(a: Platform, b: Platform): number {
  return PLATFORM_PRIORITY.indexOf(a) - PLATFORM_PRIORITY.indexOf(b);
}
