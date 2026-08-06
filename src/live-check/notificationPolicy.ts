import type { Streamer, StreamerTier } from "./streamers.js";

/**
 * Decoupled from `Pick<Streamer, "tier">` (which is non-optional) so callers
 * that don't care about tier — including old tests — can omit it and get the
 * "primary" default behavior.
 */
type StreamerLike = Pick<Streamer, "liveNotifications"> & { tier?: StreamerTier };

/**
 * Whether "live activity" notifications (went-live, title-change, went-offline)
 * are enabled for a streamer. Muted via `liveNotifications: false` or the
 * `background` tier in channels.json; defaults to enabled.
 */
export function liveNotificationsEnabled(streamer: StreamerLike): boolean {
  if (streamer.tier === "background") return false;
  return streamer.liveNotifications !== false;
}

/**
 * "all": every window (7d/30d/90d/all-time) may notify on a confirmed record.
 * "all-time-only": only the all-time window may notify; the other windows are
 * still tracked/persisted, just never pushed.
 */
export type ViewerRecordScope = "all" | "all-time-only";

export type NotificationPermissions = {
  wentLive: boolean;
  titleChange: boolean;
  wentOffline: boolean;
  /**
   * Viewer-record notifications are never muted by liveNotifications — records
   * and all tracking continue for muted streamers. The background tier
   * narrows this to all-time-only rather than muting it outright.
   */
  viewerRecords: ViewerRecordScope;
};

/**
 * Pure decision: which notification kinds may fire for a streamer. All "live
 * activity" notifications respect the per-streamer mute; went-offline
 * additionally requires the global OFFLINE_NOTIFICATIONS flag.
 */
export function getNotificationPermissions(
  streamer: StreamerLike,
  options: { offlineNotifications: boolean },
): NotificationPermissions {
  const liveActivity = liveNotificationsEnabled(streamer);
  return {
    wentLive: liveActivity,
    titleChange: liveActivity,
    wentOffline: liveActivity && options.offlineNotifications,
    viewerRecords: streamer.tier === "background" ? "all-time-only" : "all",
  };
}
