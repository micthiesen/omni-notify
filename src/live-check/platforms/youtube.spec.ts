import { describe, expect, it } from "vitest";
import { LiveStatus } from "./index.js";
import { extractLiveStatus } from "./youtube.js";

describe("extractLiveStatus", () => {
  const wrapWithYtData = (playerResponse: unknown, html = "") =>
    `<script>var ytInitialPlayerResponse = ${JSON.stringify(playerResponse)};</script>${html}`;

  const livePlayerResponse = {
    videoDetails: {
      isLive: true,
      isLiveContent: true,
    },
    microformat: {
      playerMicroformatRenderer: {
        liveBroadcastDetails: {
          isLiveNow: true,
        },
      },
    },
  };

  it("extracts the title and viewer count for an active livestream", () => {
    const html = wrapWithYtData(
      livePlayerResponse,
      [
        '<meta name="title" content="Drum &amp; Bass Non-Stop Liquid">',
        '<script>var liveData = {"viewCount":{"runs":[{"text":"12,345"}]}};</script>',
      ].join(""),
    );

    expect(extractLiveStatus(html)).toEqual({
      status: LiveStatus.Live,
      title: "Drum & Bass Non-Stop Liquid",
      viewerCount: 12345,
    });
  });

  it("returns offline when the player response is not live", () => {
    const html = wrapWithYtData({
      videoDetails: { isLive: false, isLiveContent: false },
    });

    expect(extractLiveStatus(html)).toEqual({ status: LiveStatus.Offline });
  });

  it("returns offline for a normal channel page with no player response", () => {
    const html = [
      '<script>var ytInitialData = {"contents":{}};</script>',
      "<script>loadInitialData(a.ytInitialData,a.ytInitialPlayerResponse);</script>",
      '<script>var unrelatedData = {"isLive":true,"isLiveNow":true};</script>',
    ].join("");

    expect(extractLiveStatus(html)).toEqual({ status: LiveStatus.Offline });
  });

  it("ignores scheduled streams and unrelated live markers", () => {
    const scheduledPlayerResponse = {
      videoDetails: {
        title: "LibCon",
        isLive: true,
        isLiveContent: true,
      },
      microformat: {
        playerMicroformatRenderer: {
          liveBroadcastDetails: {
            isLiveNow: false,
            startTimestamp: "2026-08-09T18:00:00Z",
          },
        },
      },
    };
    const html = wrapWithYtData(
      scheduledPlayerResponse,
      '<script>var unrelatedData = {"isLive":true,"isLiveNow":true};</script>',
    );

    expect(extractLiveStatus(html)).toEqual({ status: LiveStatus.Offline });
  });

  it("uses the active player response when unrelated scheduled data also exists", () => {
    const html = wrapWithYtData(
      livePlayerResponse,
      [
        '<script>var scheduledEvent = {"title":"LibCon","isLive":true,"liveBroadcastDetails":{"isLiveNow":false}};</script>',
        '<meta name="title" content="Whick Is Actually Live &amp; Streaming">',
        '<script>var liveData = {"viewCount":{"runs":[{"text":"4,321"}]}};</script>',
      ].join(""),
    );

    expect(extractLiveStatus(html)).toEqual({
      status: LiveStatus.Live,
      title: "Whick Is Actually Live & Streaming",
      viewerCount: 4321,
    });
  });

  it("returns unknown when ytInitialPlayerResponse is missing", () => {
    expect(extractLiveStatus("<div>Some other content</div>")).toEqual({
      status: LiveStatus.Unknown,
      error: "Response missing expected YouTube data structure",
    });
  });

  it("skips a malformed assignment when a later player response is valid", () => {
    const html = [
      '<script>var ytInitialPlayerResponse = {"videoDetails":undefined};</script>',
      wrapWithYtData(
        livePlayerResponse,
        '<meta name="title" content="Recovered Live Stream">',
      ),
    ].join("");

    expect(extractLiveStatus(html)).toEqual({
      status: LiveStatus.Live,
      title: "Recovered Live Stream",
      viewerCount: undefined,
    });
  });

  it("returns unknown when a live stream has no title meta tag", () => {
    expect(extractLiveStatus(wrapWithYtData(livePlayerResponse))).toEqual({
      status: LiveStatus.Unknown,
      error: "Live detected but failed to extract title",
    });
  });

  it("extracts the title when the page has multiple meta tags", () => {
    const html = wrapWithYtData(
      livePlayerResponse,
      [
        '<meta name="description" content="Some description">',
        '<meta name="title" content="A &quot;quoted&quot; title &amp; special characters">',
        '<meta name="keywords" content="music, chill, relax">',
      ].join(""),
    );

    expect(extractLiveStatus(html)).toEqual({
      status: LiveStatus.Live,
      title: 'A "quoted" title & special characters',
      viewerCount: undefined,
    });
  });
});
