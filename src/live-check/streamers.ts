import type { ChannelEntry } from "../utils/config.js";
import { Platform } from "./platforms/index.js";

// Tiebreak order when multiple bindings go live in the same tick.
// Earlier = higher priority. Once a primary is elected, it sticks until it goes
// offline; this list only matters for fresh go-live ticks or re-election.
export const PLATFORM_PRIORITY: readonly Platform[] = [
  Platform.YouTube,
  Platform.Twitch,
  Platform.Kick,
];

export type PlatformBinding = { platform: Platform; username: string };

export type Streamer = {
  id: string;
  displayName: string;
  bindings: PlatformBinding[];
  pushoverToken?: string;
};

export type StreamerOverride = { pushoverToken?: string };

export function normalizeId(displayName: string): string {
  return displayName.trim().toLowerCase();
}

export function buildStreamers(
  sources: [Platform, ChannelEntry[]][],
  overrides: Record<string, StreamerOverride>,
): Streamer[] {
  const byId = new Map<string, Streamer>();
  const seenBindings = new Set<string>();

  for (const [platform, entries] of sources) {
    for (const { username, displayName } of entries) {
      const bindingKey = `${platform}:${username}`;
      if (seenBindings.has(bindingKey)) {
        throw new Error(
          `Duplicate platform binding "${bindingKey}" across channel entries`,
        );
      }
      seenBindings.add(bindingKey);

      const id = normalizeId(displayName);
      let streamer = byId.get(id);
      if (!streamer) {
        streamer = { id, displayName, bindings: [] };
        byId.set(id, streamer);
      }
      streamer.bindings.push({ platform, username });
    }
  }

  for (const [key, override] of Object.entries(overrides)) {
    const streamer = byId.get(normalizeId(key));
    if (!streamer) continue;
    if (override.pushoverToken !== undefined) {
      streamer.pushoverToken = override.pushoverToken;
    }
  }

  return [...byId.values()];
}

export function comparePlatformPriority(a: Platform, b: Platform): number {
  return PLATFORM_PRIORITY.indexOf(a) - PLATFORM_PRIORITY.indexOf(b);
}
