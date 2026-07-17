import type {
  PodcastFeedback,
  PodcastRecommendationStatus,
  RecommendationFeedback,
  RecommendationStatus,
  WatchlistResult,
} from "../api";

export const REC_STATUS_LABELS: Record<RecommendationStatus, string> = {
  pending: "Pending",
  notified: "Notified",
  watched: "Watched",
  abandoned: "Abandoned",
  ignored: "Ignored",
  failed: "Failed",
};

export const REC_STATUS_ORDER: RecommendationStatus[] = [
  "notified",
  "pending",
  "watched",
  "abandoned",
  "ignored",
  "failed",
];

export const WATCHLIST_LABELS: Record<WatchlistResult, string> = {
  added: "Added to watchlist",
  already_exists: "Already on watchlist",
  available: "Available in Plex",
  error: "Watchlist error",
};

export const REC_FEEDBACK_ACTIONS: {
  value: RecommendationFeedback;
  label: string;
}[] = [
  { value: "good_pick", label: "Good pick" },
  { value: "not_for_me", label: "Not for me" },
  { value: "already_watched", label: "Already watched" },
];

export const PODCAST_STATUS_LABELS: Record<PodcastRecommendationStatus, string> = {
  pending: "Pending",
  notified: "Notified",
  listened: "Listened",
  abandoned: "Abandoned",
  ignored: "Ignored",
  failed: "Failed",
};

export const PODCAST_STATUS_ORDER: PodcastRecommendationStatus[] = [
  "notified",
  "pending",
  "listened",
  "abandoned",
  "ignored",
  "failed",
];

export const PODCAST_FEEDBACK_ACTIONS: { value: PodcastFeedback; label: string }[] = [
  { value: "good_pick", label: "Good pick" },
  { value: "not_for_me", label: "Not for me" },
];
