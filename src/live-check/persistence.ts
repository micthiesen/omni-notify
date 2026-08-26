import { Entity } from "@micthiesen/mitools/entities";
import type { PlatformBinding } from "./streamers.js";

export type StreamerStatusLive = {
  streamerId: string;
  isLive: true;
  primary: PlatformBinding;
  primaryTitle: string;
  startedAt: Date;
  maxViewerCount: number;
  /**
   * Current (not max) summed viewer count as of the most recent tick.
   * Optional: rows persisted before this field existed won't have it.
   */
  viewerCount?: number;
  /**
   * Current per-platform observations that make up viewerCount. Older rows do
   * not have this field; platform history intentionally starts when the
   * account is first observed by the new collector.
   */
  sources?: Array<{
    platform: PlatformBinding["platform"];
    username: string;
    title: string;
    viewerCount?: number;
    category?: string;
  }>;
  /**
   * The primary binding's category/game, when the platform reports one (e.g.
   * Twitch/Kick; YouTube never sets this). Optional for the same reason as
   * viewerCount.
   */
  category?: string;
};

export type StreamerStatusOffline = {
  streamerId: string;
  isLive: false;
  lastEndedAt?: Date;
  lastStartedAt?: Date;
  lastMaxViewerCount?: number;
};

export type StreamerStatus = StreamerStatusLive | StreamerStatusOffline;

export const StreamerStatusEntity = new Entity<StreamerStatus, ["streamerId"]>(
  "streamer-status",
  ["streamerId"],
);

export function getStreamerStatus(streamerId: string): StreamerStatus {
  return StreamerStatusEntity.get({ streamerId }) ?? { streamerId, isLive: false };
}

export function upsertStreamerStatus(status: StreamerStatus): void {
  StreamerStatusEntity.upsert(status);
}
