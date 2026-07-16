import { describe, expect, it } from "vitest";
import { makeEpisodeId, makeShowId, normalizeFeedUrl } from "./types.js";

describe("makeShowId", () => {
  it("prefers the iTunes id", () => {
    expect(makeShowId({ itunesId: 123, feedUrl: "https://x.com/feed" })).toBe(
      "itunes:123",
    );
  });

  it("falls back to the normalized feed url", () => {
    expect(makeShowId({ feedUrl: "HTTPS://Feeds.Example.com/GrayArea/" })).toBe(
      "feed:feeds.example.com/grayarea",
    );
  });

  it("returns undefined with no identifiers", () => {
    expect(makeShowId({})).toBeUndefined();
  });
});

describe("normalizeFeedUrl", () => {
  it("strips protocol, trailing slashes, and case", () => {
    expect(normalizeFeedUrl("http://Feeds.X.com/a/")).toBe("feeds.x.com/a");
    expect(normalizeFeedUrl("https://feeds.x.com/a")).toBe("feeds.x.com/a");
  });
});

describe("makeEpisodeId", () => {
  it("joins show id and guid", () => {
    expect(makeEpisodeId("itunes:1", "guid-9")).toBe("itunes:1#guid-9");
  });
});
