import { describe, expect, it } from "vitest";
import { LiveStatus } from "./index.js";
import { extractLiveStatus } from "./twitch.js";

describe("extractLiveStatus", () => {
  it("should prefer liveUpNotification over stream title", () => {
    const data = {
      data: {
        user: {
          stream: {
            title: "Playing games",
            viewersCount: 15000,
          },
          broadcastSettings: {
            liveUpNotification: "Custom notification message",
          },
        },
      },
    };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "Custom notification message",
      viewerCount: 15000,
    });
  });

  it("should fall back to stream title when liveUpNotification is null", () => {
    const data = {
      data: {
        user: {
          stream: {
            title: "Playing games",
            viewersCount: 15000,
          },
          broadcastSettings: {
            liveUpNotification: null,
          },
        },
      },
    };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "Playing games",
      viewerCount: 15000,
    });
  });

  it("should fall back to stream title when liveUpNotification is empty", () => {
    const data = {
      data: {
        user: {
          stream: {
            title: "Playing games",
            viewersCount: 15000,
          },
          broadcastSettings: {
            liveUpNotification: "",
          },
        },
      },
    };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "Playing games",
      viewerCount: 15000,
    });
  });

  it("should return offline when stream is null", () => {
    const data = {
      data: {
        user: {
          stream: null,
          broadcastSettings: { liveUpNotification: null },
        },
      },
    };
    expect(extractLiveStatus(data)).toEqual({ status: LiveStatus.Offline });
  });

  it("should return offline when user is null", () => {
    const data = { data: { user: null } };
    expect(extractLiveStatus(data)).toEqual({ status: LiveStatus.Offline });
  });
});
