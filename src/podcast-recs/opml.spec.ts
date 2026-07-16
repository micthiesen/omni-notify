import { describe, expect, it } from "vitest";
import { parseOpmlSubscriptions } from "./opml.js";

const CASTRO_OPML = `<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head>
    <title>Castro Subscriptions</title>
  </head>
  <body>
    <outline text="Shows" title="Shows">
      <outline text="Reply All &amp; Friends" title="Reply All &amp; Friends" type="rss" xmlUrl="https://feeds.example.com/replyall" htmlUrl="https://example.com/replyall"/>
      <outline text='Single Quoted Show' title='Single Quoted Show' type='rss' xmlUrl='https://feeds.example.com/singlequoted' htmlUrl='https://example.com/singlequoted'/>
      <outline type="rss" xmlUrl="https://feeds.example.com/notitle"/>
      <outline text="Caf&#233; Talk &#x2013; Live" type="rss" xmlUrl="https://feeds.example.com/cafe"/>
    </outline>
    <outline text="Duplicates" title="Duplicates">
      <outline text="Reply All Dup" type="rss" xmlUrl="https://feeds.example.com/replyall"/>
    </outline>
  </body>
</opml>`;

describe("parseOpmlSubscriptions", () => {
  it("parses nested outlines with xmlUrl, skipping folder outlines", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);

    // 4 unique feeds: replyall, singlequoted, notitle, cafe (dup replyall excluded)
    expect(result).toHaveLength(4);
  });

  it("decodes entity-encoded titles (&amp;)", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);
    const replyAll = result.find(
      (s) => s.feedUrl === "https://feeds.example.com/replyall",
    );
    expect(replyAll).toBeDefined();
    expect(replyAll?.title).toBe("Reply All & Friends");
  });

  it("decodes numeric XML entities (decimal and hex)", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);
    const cafe = result.find((s) => s.feedUrl === "https://feeds.example.com/cafe");
    expect(cafe).toBeDefined();
    expect(cafe?.title).toBe("Café Talk – Live");
  });

  it("supports single-quoted attributes", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);
    const single = result.find(
      (s) => s.feedUrl === "https://feeds.example.com/singlequoted",
    );
    expect(single).toBeDefined();
    expect(single?.title).toBe("Single Quoted Show");
  });

  it("falls back to the feed URL host when no text/title attribute is present", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);
    const noTitle = result.find(
      (s) => s.feedUrl === "https://feeds.example.com/notitle",
    );
    expect(noTitle).toBeDefined();
    expect(noTitle?.title).toBe("feeds.example.com");
  });

  it("dedupes by feedUrl, keeping the first occurrence", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);
    const matches = result.filter(
      (s) => s.feedUrl === "https://feeds.example.com/replyall",
    );
    expect(matches).toHaveLength(1);
    expect(matches[0]?.title).toBe("Reply All & Friends");
  });

  it("ignores folder/group outlines that have no xmlUrl", () => {
    const result = parseOpmlSubscriptions(CASTRO_OPML);
    expect(result.some((s) => s.title === "Shows")).toBe(false);
    expect(result.some((s) => s.title === "Duplicates")).toBe(false);
  });

  it("returns [] for empty input", () => {
    expect(parseOpmlSubscriptions("")).toEqual([]);
  });

  it("returns [] for malformed/non-OPML input instead of throwing", () => {
    expect(() => parseOpmlSubscriptions("not xml at all <<<>>>")).not.toThrow();
    expect(parseOpmlSubscriptions("not xml at all <<<>>>")).toEqual([]);
    expect(parseOpmlSubscriptions("<html><body>hello</body></html>")).toEqual([]);
  });
});
