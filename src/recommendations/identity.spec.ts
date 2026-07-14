import { describe, expect, it } from "vitest";
import { parseGuidExternalIds, scoreSearchResults } from "./identity.js";
import { MediaType } from "./types.js";

describe("parseGuidExternalIds", () => {
  it("parses plain tmdb guids", () => {
    expect(parseGuidExternalIds("tmdb://12345")).toEqual({ tmdb: 12345 });
  });

  it("parses plain imdb guids", () => {
    expect(parseGuidExternalIds("imdb://tt0111161")).toEqual({ imdb: "tt0111161" });
  });

  it("parses plain tvdb guids", () => {
    expect(parseGuidExternalIds("tvdb://81189")).toEqual({ tvdb: 81189 });
  });

  it("parses legacy agent guids with query strings", () => {
    expect(parseGuidExternalIds("com.plexapp.agents.imdb://tt0111161?lang=en")).toEqual(
      { imdb: "tt0111161" },
    );
    expect(parseGuidExternalIds("com.plexapp.agents.themoviedb://603?lang=en")).toEqual(
      { tmdb: 603 },
    );
    expect(
      parseGuidExternalIds("com.plexapp.agents.thetvdb://81189/3/7?lang=en"),
    ).toEqual({ tvdb: 81189 });
  });

  it("returns empty for opaque guids", () => {
    expect(parseGuidExternalIds("plex://movie/5d7768ba96b655001fdc0408")).toEqual({});
    expect(parseGuidExternalIds("local://12345")).toEqual({});
  });
});

describe("scoreSearchResults", () => {
  const item = { title: "The Thing", year: 1982, mediaType: MediaType.Movie };

  it("accepts a single title+year match with high confidence", () => {
    const result = scoreSearchResults(item, [
      { tmdbId: 1091, title: "The Thing", year: 1982, voteCount: 8000 },
      { tmdbId: 60935, title: "The Thing", year: 2011, voteCount: 3000 },
    ]);
    expect(result?.canonicalId).toBe("tmdb:movie:1091");
    expect(result?.confidence).toBeGreaterThanOrEqual(0.95);
  });

  it("tolerates off-by-one release years", () => {
    const result = scoreSearchResults(item, [
      { tmdbId: 1091, title: "The Thing", year: 1981, voteCount: 8000 },
    ]);
    expect(result?.canonicalId).toBe("tmdb:movie:1091");
  });

  it("returns unresolved for ambiguous matches with similar vote counts", () => {
    const result = scoreSearchResults(
      { title: "The Thing", mediaType: MediaType.Movie },
      [
        { tmdbId: 1091, title: "The Thing", year: 1982, voteCount: 8000 },
        { tmdbId: 60935, title: "The Thing", year: 2011, voteCount: 3000 },
      ],
    );
    expect(result?.canonicalId).toBeNull();
  });

  it("accepts a dominant match when one result dwarfs the rest", () => {
    const result = scoreSearchResults(
      { title: "The Thing", mediaType: MediaType.Movie },
      [
        { tmdbId: 1091, title: "The Thing", year: 1982, voteCount: 50000 },
        { tmdbId: 99999, title: "The Thing", year: 2005, voteCount: 12 },
      ],
    );
    expect(result?.canonicalId).toBe("tmdb:movie:1091");
  });

  it("normalizes punctuation and accents in titles", () => {
    const result = scoreSearchResults(
      { title: "Amelie", year: 2001, mediaType: MediaType.Movie },
      [{ tmdbId: 194, title: "Amélie", year: 2001, voteCount: 11000 }],
    );
    expect(result?.canonicalId).toBe("tmdb:movie:194");
  });

  it("returns undefined when nothing matches the title", () => {
    const result = scoreSearchResults(item, [
      { tmdbId: 1, title: "Something Else", year: 1982, voteCount: 100 },
    ]);
    expect(result).toBeUndefined();
  });
});
