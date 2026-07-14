import { describe, expect, it } from "vitest";
import { MediaType } from "../types.js";
import {
  findResponseSchema,
  movieListSchema,
  normalizeMovie,
  normalizeTv,
  trendingListSchema,
} from "./types.js";

describe("normalizeMovie", () => {
  it("normalizes a full movie payload", () => {
    const parsed = movieListSchema.parse({
      results: [
        {
          id: 603,
          title: "The Matrix",
          release_date: "1999-03-30",
          overview: "A computer hacker...",
          genre_ids: [28, 878],
          vote_average: 8.2,
          vote_count: 26000,
          popularity: 88.5,
          poster_path: "/abc.jpg",
        },
      ],
    });
    const title = normalizeMovie(parsed.results[0]);
    expect(title).toEqual({
      tmdbId: 603,
      mediaType: MediaType.Movie,
      title: "The Matrix",
      year: 1999,
      overview: "A computer hacker...",
      genreIds: [28, 878],
      voteAverage: 8.2,
      voteCount: 26000,
      popularity: 88.5,
      posterPath: "/abc.jpg",
    });
  });

  it("handles missing optional fields and empty release dates", () => {
    const parsed = movieListSchema.parse({
      results: [{ id: 1, title: "Obscure", release_date: "" }],
    });
    const title = normalizeMovie(parsed.results[0]);
    expect(title.year).toBeUndefined();
    expect(title.overview).toBe("");
    expect(title.genreIds).toEqual([]);
    expect(title.posterPath).toBeUndefined();
  });
});

describe("normalizeTv", () => {
  it("uses name and first_air_date", () => {
    const title = normalizeTv({
      id: 1396,
      name: "Breaking Bad",
      first_air_date: "2008-01-20",
      overview: "",
      genre_ids: [18],
      vote_average: 8.9,
      vote_count: 12000,
      popularity: 200,
      poster_path: null,
      adult: false,
    });
    expect(title.mediaType).toBe(MediaType.Tv);
    expect(title.title).toBe("Breaking Bad");
    expect(title.year).toBe(2008);
    expect(title.posterPath).toBeUndefined();
  });
});

describe("trendingListSchema", () => {
  it("tolerates person entries in trending results", () => {
    const parsed = trendingListSchema.parse({
      results: [
        { media_type: "movie", id: 1, title: "A Movie" },
        { media_type: "person", id: 2, name: "An Actor" },
        { media_type: "tv", id: 3, name: "A Show" },
      ],
    });
    expect(parsed.results).toHaveLength(3);
  });
});

describe("findResponseSchema", () => {
  it("defaults missing result arrays", () => {
    const parsed = findResponseSchema.parse({ movie_results: [] });
    expect(parsed.tv_results).toEqual([]);
  });
});
