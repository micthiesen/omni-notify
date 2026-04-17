import { describe, expect, it } from "vitest";
import { LiveStatus } from "./index.js";
import { extractLiveStatus, type KickChannelsResponse } from "./kick.js";

const baseChannel: KickChannelsResponse["data"][number] = {
  slug: "destiny",
  stream_title: "ultra boring work/emails",
  category: { id: 15, name: "Just Chatting" },
  stream: { is_live: true, viewer_count: 3952 },
};

describe("extractLiveStatus", () => {
  it("returns live with title, viewers, and category when is_live is true", () => {
    const data: KickChannelsResponse = { data: [baseChannel], message: "success" };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "ultra boring work/emails",
      viewerCount: 3952,
      category: "Just Chatting",
    });
  });

  it("returns offline when the stream object is missing", () => {
    const data: KickChannelsResponse = {
      data: [{ ...baseChannel, stream: null }],
    };
    expect(extractLiveStatus(data)).toEqual({ status: LiveStatus.Offline });
  });

  it("returns offline when is_live is false", () => {
    const data: KickChannelsResponse = {
      data: [{ ...baseChannel, stream: { is_live: false } }],
    };
    expect(extractLiveStatus(data)).toEqual({ status: LiveStatus.Offline });
  });

  it("returns offline when no channel matches the slug", () => {
    const data: KickChannelsResponse = { data: [] };
    expect(extractLiveStatus(data)).toEqual({ status: LiveStatus.Offline });
  });

  it("falls back to slug when stream_title is empty", () => {
    const data: KickChannelsResponse = {
      data: [{ ...baseChannel, stream_title: "" }],
    };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "destiny",
      viewerCount: 3952,
      category: "Just Chatting",
    });
  });

  it("omits category when not present", () => {
    const data: KickChannelsResponse = {
      data: [{ ...baseChannel, category: null }],
    };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "ultra boring work/emails",
      viewerCount: 3952,
      category: undefined,
    });
  });

  it("omits viewer count when API omits it", () => {
    const data: KickChannelsResponse = {
      data: [{ ...baseChannel, stream: { is_live: true } }],
    };
    expect(extractLiveStatus(data)).toEqual({
      status: LiveStatus.Live,
      title: "ultra boring work/emails",
      viewerCount: undefined,
      category: "Just Chatting",
    });
  });
});
