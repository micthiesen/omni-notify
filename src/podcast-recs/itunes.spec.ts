import { describe, expect, it } from "vitest";
import { type ItunesShow, pickBestShowMatch } from "./itunes.js";

function show(overrides: Partial<ItunesShow> & { itunesId: number }): ItunesShow {
  return { title: "Untitled", genres: [], ...overrides };
}

describe("pickBestShowMatch", () => {
  it("picks the exact normalized match", () => {
    const shows = [
      show({ itunesId: 1, title: "The Daily" }),
      show({ itunesId: 2, title: "Reply All" }),
    ];
    expect(pickBestShowMatch(shows, "reply all")).toEqual(shows[1]);
  });

  it("matches despite punctuation and casing differences", () => {
    const shows = [show({ itunesId: 1, title: "Radio Lab" })];
    expect(pickBestShowMatch(shows, "radio-lab!!")).toEqual(shows[0]);
  });

  it("matches despite diacritics", () => {
    const shows = [show({ itunesId: 1, title: "Café Society" })];
    expect(pickBestShowMatch(shows, "cafe society")).toEqual(shows[0]);
  });

  it("falls back to containment when the query is a prefix of the title", () => {
    const shows = [show({ itunesId: 1, title: "Reply All: The Podcast" })];
    expect(pickBestShowMatch(shows, "Reply All")).toEqual(shows[0]);
  });

  it("falls back to containment when the title is a prefix of the query", () => {
    const shows = [show({ itunesId: 1, title: "Reply All" })];
    expect(pickBestShowMatch(shows, "Reply All: The Podcast")).toEqual(shows[0]);
  });

  it("returns undefined when nothing matches", () => {
    const shows = [show({ itunesId: 1, title: "The Daily" })];
    expect(pickBestShowMatch(shows, "Completely Unrelated Show")).toBeUndefined();
  });

  it("returns undefined for an empty shows list", () => {
    expect(pickBestShowMatch([], "Anything")).toBeUndefined();
  });
});
