import type { Streamer } from "./streamers.js";

/**
 * Whether "live activity" notifications (went-live, title-change, went-offline)
 * are enabled for a streamer. Muted via `liveNotifications: false` in
 * channels.json; defaults to enabled.
 */
export function liveNotificationsEnabled(
  streamer: Pick<Streamer, "liveNotifications">,
): boolean {
  return streamer.liveNotifications !== false;
}

export type NotificationPermissions = {
  wentLive: boolean;
  titleChange: boolean;
  wentOffline: boolean;
  /**
   * Viewer-record notifications are never muted by liveNotifications — records
   * and all tracking continue for muted streamers.
   */
  viewerRecords: true;
};

/**
 * Pure decision: which notification kinds may fire for a streamer. All "live
 * activity" notifications respect the per-streamer mute; went-offline
 * additionally requires the global OFFLINE_NOTIFICATIONS flag.
 */
export function getNotificationPermissions(
  streamer: Pick<Streamer, "liveNotifications">,
  options: { offlineNotifications: boolean },
): NotificationPermissions {
  const liveActivity = liveNotificationsEnabled(streamer);
  return {
    wentLive: liveActivity,
    titleChange: liveActivity,
    wentOffline: liveActivity && options.offlineNotifications,
    viewerRecords: true,
  };
}
