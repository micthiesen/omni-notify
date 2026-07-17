import { describe, expect, it } from "vitest";
import {
  getNotificationPermissions,
  liveNotificationsEnabled,
} from "./notificationPolicy.js";

describe("liveNotificationsEnabled", () => {
  it("defaults to enabled when liveNotifications is undefined", () => {
    expect(liveNotificationsEnabled({})).toBe(true);
  });

  it("is enabled when liveNotifications is explicitly true", () => {
    expect(liveNotificationsEnabled({ liveNotifications: true })).toBe(true);
  });

  it("is disabled when liveNotifications is false", () => {
    expect(liveNotificationsEnabled({ liveNotifications: false })).toBe(false);
  });
});

describe("getNotificationPermissions", () => {
  const offlineOn = { offlineNotifications: true };
  const offlineOff = { offlineNotifications: false };

  it("permits all live-activity notifications for a default streamer", () => {
    expect(getNotificationPermissions({}, offlineOn)).toEqual({
      wentLive: true,
      titleChange: true,
      wentOffline: true,
      viewerRecords: true,
    });
  });

  it("mutes went-live, title-change, and went-offline when liveNotifications is false", () => {
    expect(getNotificationPermissions({ liveNotifications: false }, offlineOn)).toEqual(
      {
        wentLive: false,
        titleChange: false,
        wentOffline: false,
        viewerRecords: true,
      },
    );
  });

  it("still permits viewer-record notifications for muted streamers", () => {
    const permissions = getNotificationPermissions(
      { liveNotifications: false },
      offlineOn,
    );
    expect(permissions.viewerRecords).toBe(true);
  });

  it("suppresses went-offline when OFFLINE_NOTIFICATIONS is disabled globally", () => {
    const permissions = getNotificationPermissions({}, offlineOff);
    expect(permissions.wentOffline).toBe(false);
    expect(permissions.wentLive).toBe(true);
    expect(permissions.titleChange).toBe(true);
    expect(permissions.viewerRecords).toBe(true);
  });
});
