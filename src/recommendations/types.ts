export enum MediaType {
  Movie = "movie",
  Tv = "tv",
}

/** Canonical content identity: TMDB id + media type. */
export type CanonicalId = `tmdb:${MediaType}:${number}`;

export function makeCanonicalId(mediaType: MediaType, tmdbId: number): CanonicalId {
  return `tmdb:${mediaType}:${tmdbId}`;
}

export interface ExternalIds {
  tmdb?: number;
  imdb?: string;
  tvdb?: number;
}

/** An item as known to the local media library / watchlist service. */
export interface MediaItem {
  /** Opaque server-native id (e.g. a Plex GUID). Stable per backend. */
  guid: string;
  title: string;
  year?: number;
  mediaType: MediaType;
  externalIds?: ExternalIds;
}

export interface WatchedItem extends MediaItem {
  viewedAt: number;
  viewCount: number;
  /** 0-1 fraction of runtime watched, when the backend reports it. */
  completion?: number;
}

export interface InProgressItem extends MediaItem {
  /** 0-1 fraction of runtime watched. */
  progress: number;
  lastViewedAt: number;
}

export type { FetchResult } from "../utils/fetchResult.js";

export type AddToWatchlistResult =
  | "added"
  | "already_exists"
  | "not_found"
  | "unavailable"
  | "error";

/** A candidate title assembled from TMDB, pre-scoring. */
export interface Candidate {
  canonicalId: CanonicalId;
  tmdbId: number;
  mediaType: MediaType;
  title: string;
  year?: number;
  overview: string;
  genres: string[];
  voteAverage: number;
  voteCount: number;
  popularity: number;
  posterPath?: string;
  /** Typical runtime for a movie or episode, when TMDB reports it. */
  runtimeMinutes?: number;
  /** TV commitment information. */
  seasonCount?: number;
  episodeCount?: number;
  seriesStatus?: string;
  originalLanguage?: string;
  originCountries?: string[];
  creators?: string[];
  cast?: string[];
  keywords?: string[];
  /** US content certification (for example PG-13 or TV-MA), when available. */
  certification?: string;
  source: CandidateSource;
  /** Present in the local media library (positive signal, not an exclusion). */
  inLibrary: boolean;
}

export enum CandidateSource {
  Similar = "similar",
  Discover = "discover",
  Trending = "trending",
  Novelty = "novelty",
}
