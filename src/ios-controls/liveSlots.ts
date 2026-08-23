import { createHash } from "node:crypto";
import { sortLiveDisplay } from "../live-check/displayOrder.js";
import { getStreamerStatus } from "../live-check/persistence.js";
import { type Platform, platformConfigs } from "../live-check/platforms/index.js";
import { type Streamer, streamerOrderingViewerCount } from "../live-check/streamers.js";

export const IOS_CONTROL_SLOT_COUNT = 4;

export type LiveControlSlot = {
  slot: number;
  isLive: boolean;
  streamerId: string | null;
  displayName: string;
  title: string | null;
  platform: Platform | null;
  url: string;
  viewerCount: number | null;
  startedAt: number | null;
  updatedAt: number;
};

type RankedLiveSlot = Omit<LiveControlSlot, "slot" | "updatedAt"> & {
  tier: Streamer["tier"];
  maxViewerCount: number;
  orderingViewerCount?: number;
};

function epoch(value: Date | string): number {
  return new Date(value).getTime();
}

/**
 * Server-side equivalent of the dashboard's "primary first, then hottest"
 * ordering. JavaScript's stable sort preserves channels.json order for ties,
 * matching the frontend's stable sort behavior.
 */
export function buildLiveControlSlots(
  streamers: Streamer[],
  homeUrl: string,
  now = Date.now(),
): LiveControlSlot[] {
  const ranked: RankedLiveSlot[] = [];
  for (const streamer of streamers) {
    const status = getStreamerStatus(streamer.id);
    if (!status.isLive) continue;
    const viewerCount = status.viewerCount ?? null;
    ranked.push({
      tier: streamer.tier,
      maxViewerCount: status.maxViewerCount,
      orderingViewerCount: streamerOrderingViewerCount(streamer),
      isLive: true,
      streamerId: streamer.id,
      displayName: streamer.displayName,
      title: status.primaryTitle,
      platform: status.primary.platform,
      url:
        status.primary.urlOverride ??
        platformConfigs[status.primary.platform].getLiveUrl(status.primary.username),
      viewerCount,
      startedAt: epoch(status.startedAt),
    });
  }

  const ordered = sortLiveDisplay(ranked);

  return Array.from({ length: IOS_CONTROL_SLOT_COUNT }, (_, index) => {
    const live = ordered[index];
    if (live) {
      const {
        tier: _tier,
        maxViewerCount: _maxViewerCount,
        orderingViewerCount: _orderingViewerCount,
        ...slot
      } = live;
      return { ...slot, slot: index + 1, updatedAt: now };
    }
    return {
      slot: index + 1,
      isLive: false,
      streamerId: null,
      displayName: "Nobody Live",
      title: null,
      platform: null,
      url: homeUrl,
      viewerCount: null,
      startedAt: null,
      updatedAt: now,
    };
  });
}

/** Hash only user-visible/control-action state, not the response timestamp. */
export function liveControlSlotHash(slot: LiveControlSlot): string {
  const state = {
    slot: slot.slot,
    isLive: slot.isLive,
    streamerId: slot.streamerId,
    displayName: slot.displayName,
    title: slot.title,
    url: slot.url,
  };
  return createHash("sha256").update(JSON.stringify(state)).digest("hex");
}
