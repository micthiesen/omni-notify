import { describe, expect, it } from "vitest";
import { MediaType } from "../types.js";
import {
  findResponseSchema,
  movieDetailsSchema,
  movieListSchema,
  normalizeMovie,
  normalizeMovieDetails,
  normalizeTv,
  normalizeTvDetails,
  trendingListSchema,
  tvDetailsSchema,
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
          original_language: "en",
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
      originalLanguage: "en",
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
      original_language: "en",
      adult: false,
    });
    expect(title.mediaType).toBe(MediaType.Tv);
    expect(title.title).toBe("Breaking Bad");
    expect(title.year).toBe(2008);
    expect(title.posterPath).toBeUndefined();
    expect(title.originalLanguage).toBe("en");
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

describe("structured title details", () => {
  it("normalizes movie commitment and creative metadata", () => {
    const parsed = movieDetailsSchema.parse({
      genres: [{ id: 878, name: "Science Fiction" }],
      runtime: 136,
      original_language: "fr",
      origin_country: ["FR"],
      credits: {
        cast: [{ name: "Lead Actor" }, { name: "Second Actor" }],
        crew: [
          { name: "The Director", job: "Director" },
          { name: "The Writer", job: "Writer" },
        ],
      },
      keywords: { keywords: [{ name: "time travel" }, { name: "memory" }] },
      release_dates: {
        results: [
          {
            iso_3166_1: "US",
            release_dates: [{ certification: "" }, { certification: "PG-13" }],
          },
        ],
      },
    });

    expect(normalizeMovieDetails(parsed)).toEqual({
      genres: ["Science Fiction"],
      runtimeMinutes: 136,
      originalLanguage: "fr",
      originCountries: ["FR"],
      creators: ["The Director"],
      cast: ["Lead Actor", "Second Actor"],
      keywords: ["time travel", "memory"],
      certification: "PG-13",
    });
  });

  it("normalizes TV series size and uses the median episode runtime", () => {
    const parsed = tvDetailsSchema.parse({
      genres: [],
      episode_run_time: [60, 30, 45],
      number_of_seasons: 3,
      number_of_episodes: 24,
      status: "Ended",
      original_language: "en",
      origin_country: ["US"],
      created_by: [{ name: "A Creator" }],
      credits: { cast: [{ name: "The Star" }] },
      keywords: { results: [{ name: "workplace" }] },
      content_ratings: {
        results: [{ iso_3166_1: "US", rating: "TV-MA" }],
      },
    });

    expect(normalizeTvDetails(parsed)).toEqual({
      genres: [],
      runtimeMinutes: 45,
      seasonCount: 3,
      episodeCount: 24,
      seriesStatus: "Ended",
      originalLanguage: "en",
      originCountries: ["US"],
      creators: ["A Creator"],
      cast: ["The Star"],
      keywords: ["workplace"],
      certification: "TV-MA",
    });
  });
});
