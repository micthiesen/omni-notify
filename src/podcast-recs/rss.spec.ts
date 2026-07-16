import { describe, expect, it } from "vitest";
import { findEpisodeByTitle, parseFeedEpisodes } from "./rss.js";

const FIXTURE_XML = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
<channel>
<title>Test Podcast</title>
<item>
  <title><![CDATA[Episode One: Tom &amp; Jerry's "Big" Day]]></title>
  <guid isPermaLink="false">guid-episode-one</guid>
  <pubDate>Thu, 02 Jan 2025 12:00:00 GMT</pubDate>
  <itunes:duration>1:02:03</itunes:duration>
  <description><![CDATA[<p>An episode about &amp; things.</p><p>Extra   spaces   here.</p>]]></description>
  <link>https://example.com/episodes/one</link>
</item>
<item>
  <title>Episode Two: Seconds Duration</title>
  <guid isPermaLink="false">guid-episode-two</guid>
  <pubDate>Fri, 03 Jan 2025 12:00:00 GMT</pubDate>
  <itunes:duration>3720</itunes:duration>
  <itunes:summary>Summary only, no description tag.</itunes:summary>
</item>
<item>
  <title>Episode Three: No Guid, Has Enclosure</title>
  <enclosure url="https://example.com/audio/three.mp3" length="123" type="audio/mpeg"/>
  <pubDate>Sat, 04 Jan 2025 12:00:00 GMT</pubDate>
</item>
<item>
  <title>Episode Four: Invalid Date Is Skipped</title>
  <guid>guid-episode-four</guid>
  <pubDate>not-a-real-date</pubDate>
</item>
<item>
  <title>Episode Five: No Duration Tag</title>
  <guid>guid-episode-five</guid>
  <pubDate>Sun, 05 Jan 2025 12:00:00 GMT</pubDate>
</item>
</channel>
</rss>`;

describe("parseFeedEpisodes", () => {
  const episodes = parseFeedEpisodes(FIXTURE_XML);

  it("skips items with an unparseable pubDate", () => {
    expect(episodes.some((e) => e.title.includes("Invalid Date"))).toBe(false);
  });

  it("parses the expected number of valid episodes", () => {
    expect(episodes).toHaveLength(4);
  });

  it("decodes CDATA-wrapped, entity-encoded titles", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-one");
    expect(episode?.title).toBe(`Episode One: Tom & Jerry's "Big" Day`);
  });

  it("parses itunes:duration in HH:MM:SS form", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-one");
    expect(episode?.durationMinutes).toBe(62);
  });

  it("parses itunes:duration given as plain seconds", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-two");
    expect(episode?.durationMinutes).toBe(62);
  });

  it("strips HTML tags, decodes entities, and collapses whitespace in the description", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-one");
    expect(episode?.description).toBe("An episode about & things. Extra spaces here.");
  });

  it("falls back to itunes:summary when description is missing", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-two");
    expect(episode?.description).toBe("Summary only, no description tag.");
  });

  it("falls back to the enclosure url when guid is missing", () => {
    const episode = episodes.find(
      (e) => e.guid === "https://example.com/audio/three.mp3",
    );
    expect(episode).toBeDefined();
    expect(episode?.title).toBe("Episode Three: No Guid, Has Enclosure");
  });

  it("omits durationMinutes when itunes:duration is absent", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-five");
    expect(episode?.durationMinutes).toBeUndefined();
  });

  it("captures the link element when present", () => {
    const episode = episodes.find((e) => e.guid === "guid-episode-one");
    expect(episode?.link).toBe("https://example.com/episodes/one");
  });

  it("sorts episodes newest first", () => {
    const dates = episodes.map((e) => e.publishedAt);
    expect(dates).toEqual([...dates].sort((a, b) => b - a));
  });

  it("caps results at maxEpisodes", () => {
    const capped = parseFeedEpisodes(FIXTURE_XML, 2);
    expect(capped).toHaveLength(2);
  });
});

describe("findEpisodeByTitle", () => {
  const episodes = parseFeedEpisodes(FIXTURE_XML);

  it("finds an exact normalized match", () => {
    const found = findEpisodeByTitle(episodes, "episode two seconds duration");
    expect(found?.guid).toBe("guid-episode-two");
  });

  it("finds the longest containment match when the query is a substring", () => {
    const found = findEpisodeByTitle(episodes, "Seconds Duration");
    expect(found?.guid).toBe("guid-episode-two");
  });

  it("finds a match when the query is longer than the title", () => {
    const found = findEpisodeByTitle(
      episodes,
      "Episode Two: Seconds Duration (Director's Cut)",
    );
    expect(found?.guid).toBe("guid-episode-two");
  });

  it("returns undefined when nothing matches", () => {
    expect(findEpisodeByTitle(episodes, "Totally Unrelated Title")).toBeUndefined();
  });

  it("returns undefined for an empty episode list", () => {
    expect(findEpisodeByTitle([], "Anything")).toBeUndefined();
  });
});
