import { Effect } from "effect";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { LiveStatus } from "./index.js";
import {
  extractLiveStatus,
  fetchKickLiveStatus,
  type KickChannelsResponse,
  type KickHttpRequest,
} from "./kick.js";

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

describe("bounded Kick responses", () => {
  const originalClientId = process.env.KICK_CLIENT_ID;
  const originalClientSecret = process.env.KICK_CLIENT_SECRET;

  beforeAll(() => {
    process.env.KICK_CLIENT_ID = "client";
    process.env.KICK_CLIENT_SECRET = "secret";
  });

  afterAll(() => {
    if (originalClientId === undefined) delete process.env.KICK_CLIENT_ID;
    else process.env.KICK_CLIENT_ID = originalClientId;
    if (originalClientSecret === undefined) delete process.env.KICK_CLIENT_SECRET;
    else process.env.KICK_CLIENT_SECRET = originalClientSecret;
  });

  it("rejects a fixed-length oversized token response before buffering", async () => {
    const destroy = vi.fn();
    const request = vi.fn(() => ({
      response: { statusCode: 200, headers: { "content-length": "9" } },
      destroy,
      async *[Symbol.asyncIterator]() {},
    })) as KickHttpRequest;

    const result = await Effect.runPromise(
      fetchKickLiveStatus(
        { username: "destiny" },
        { request, tokenMaxResponseBytes: 8 },
      ),
    );

    expect(result).toMatchObject({
      status: LiveStatus.Unknown,
      error: expect.stringContaining("Response exceeds the 8-byte limit"),
    });
    expect(destroy).toHaveBeenCalledOnce();
  });

  it("cancels a chunked oversized channels response", async () => {
    const destroy = vi.fn();
    const request = vi.fn((url: string | URL) => {
      const token = String(url).includes("oauth/token");
      return {
        response: { statusCode: 200, headers: {} },
        destroy: token ? vi.fn() : destroy,
        async *[Symbol.asyncIterator]() {
          if (token) {
            yield JSON.stringify({
              access_token: "token",
              token_type: "Bearer",
              expires_in: 3600,
            });
          } else {
            yield "12345";
            yield "67890";
          }
        },
      };
    }) as KickHttpRequest;

    const result = await Effect.runPromise(
      fetchKickLiveStatus(
        { username: "destiny" },
        { request, channelsMaxResponseBytes: 8 },
      ),
    );

    expect(result).toMatchObject({
      status: LiveStatus.Unknown,
      error: expect.stringContaining("Response exceeds the 8-byte limit"),
    });
    expect(destroy).toHaveBeenCalledOnce();
  });
});
