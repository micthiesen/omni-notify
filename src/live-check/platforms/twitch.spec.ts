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
            game: { name: "Elden Ring" },
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
      category: "Elden Ring",
    });
  });

  it("should fall back to stream title when liveUpNotification is null", () => {
    const data = {
      data: {
        user: {
          stream: {
            title: "Playing games",
            viewersCount: 15000,
            game: { name: "Just Chatting" },
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
      category: "Just Chatting",
    });
  });

  it("should fall back to stream title when liveUpNotification is empty", () => {
    const data = {
      data: {
        user: {
          stream: {
            title: "Playing games",
            viewersCount: 15000,
            game: null,
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
      category: undefined,
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
