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
      viewerRecords: "all",
    });
  });

  it("mutes went-live, title-change, and went-offline when liveNotifications is false", () => {
    expect(getNotificationPermissions({ liveNotifications: false }, offlineOn)).toEqual(
      {
        wentLive: false,
        titleChange: false,
        wentOffline: false,
        viewerRecords: "all",
      },
    );
  });

  it("still permits viewer-record notifications (all windows) for muted streamers", () => {
    const permissions = getNotificationPermissions(
      { liveNotifications: false },
      offlineOn,
    );
    expect(permissions.viewerRecords).toBe("all");
  });

  it("suppresses went-offline when OFFLINE_NOTIFICATIONS is disabled globally", () => {
    const permissions = getNotificationPermissions({}, offlineOff);
    expect(permissions.wentOffline).toBe(false);
    expect(permissions.wentLive).toBe(true);
    expect(permissions.titleChange).toBe(true);
    expect(permissions.viewerRecords).toBe("all");
  });

  it("mutes all live-activity notifications for the background tier", () => {
    expect(getNotificationPermissions({ tier: "background" }, offlineOn)).toEqual({
      wentLive: false,
      titleChange: false,
      wentOffline: false,
      viewerRecords: "all-time-only",
    });
  });

  it("restricts viewer records to all-time-only for the background tier", () => {
    const permissions = getNotificationPermissions({ tier: "background" }, offlineOn);
    expect(permissions.viewerRecords).toBe("all-time-only");
  });

  it("keeps all-window viewer records for the primary tier", () => {
    const permissions = getNotificationPermissions({ tier: "primary" }, offlineOn);
    expect(permissions.viewerRecords).toBe("all");
  });
});

describe("liveNotificationsEnabled with tier", () => {
  it("is disabled for the background tier even without liveNotifications set", () => {
    expect(liveNotificationsEnabled({ tier: "background" })).toBe(false);
  });

  it("is enabled for the primary tier by default", () => {
    expect(liveNotificationsEnabled({ tier: "primary" })).toBe(true);
  });
});
