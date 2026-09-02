import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option } from "effect";
import { toDateStamp } from "../utils/dates.js";
import type { CanonicalEpisodeId, CanonicalShowId } from "./types.js";

export enum PodcastRecommendationStatus {
  /** Row written, notification not yet confirmed. */
  Pending = "pending",
  /** Notification sent; awaiting an outcome. */
  Notified = "notified",
  /** Listened past the completion threshold (requires listen-history data). */
  Listened = "listened",
  /** Started but bailed below the completion threshold. */
  Abandoned = "abandoned",
  /** No engagement within the ignore window. */
  Ignored = "ignored",
  /** Run died between the pending write and notification; reconciled later. */
  Failed = "failed",
}

export type PodcastFeedback = "good_pick" | "not_for_me";

/**
 * Outcome of the Castro auto-enqueue at commit time. `not_queued` collapses
 * every non-success (no account, show/episode unresolvable, API error) — the
 * notification deep link is the fallback in all of those cases.
 */
export type PodcastQueueResult = "queued" | "already_queued" | "not_queued";

export type PodcastRecommendationData = {
  recommendationId: string;
  episodeId: CanonicalEpisodeId;
  showId: CanonicalShowId;
  showTitle: string;
  episodeTitle: string;
  feedUrl: string;
  itunesId?: number;
  artworkUrl?: string;
  episodeGuid: string;
  mediaUrl?: string;
  episodeUrl?: string;
  publishedAt: number;
  durationMinutes?: number;
  status: PodcastRecommendationStatus;
  whyForUser?: string;
  caveats?: string[];
  confidence?: number;
  /** Selection-time evidence retained for later audit. */
  showGenres?: string[];
  discoveredVia?: string;
  sourceUrl?: string;
  /** Followed voices featured as guests (Tier-1 guest-appearance picks). */
  matchedVoices?: string[];
  shortlistScores?: {
    tasteMatch: number;
    novelty: number;
    composite: number;
    risks: string[];
  };
  /** Local date (YYYY-MM-DD) of the run that produced this recommendation. */
  runDate: string;
  recommendedAt: number;
  notifiedAt?: number;
  /** Whether the episode was placed in the Castro queue at commit time. */
  queueResult?: PodcastQueueResult;
  /** When a terminal outcome (listened/abandoned/ignored) was assigned. */
  resolvedAt?: number;
  feedback?: PodcastFeedback;
  feedbackAt?: number;
  /** Optional free-form note alongside (or instead of) the binary feedback. */
  feedbackNote?: string;
};

export const PodcastRecommendationEntity = new Entity<
  PodcastRecommendationData,
  ["recommendationId"]
>("podcast-recommendation-attempt", ["recommendationId"]);

export const getAllPodcastRecommendations = Effect.fn("PodcastRecommendations.getAll")(
  function* () {
    return (yield* PodcastRecommendationEntity.getAll()).sort(
      (a, b) => b.recommendedAt - a.recommendedAt,
    );
  },
);

export const getPodcastRecommendation = Effect.fn("PodcastRecommendations.get")(
  function* (recommendationId: string) {
    return Option.getOrUndefined(
      yield* PodcastRecommendationEntity.get({ recommendationId }),
    );
  },
);

/** Same-show cooldown; episodes themselves are excluded permanently. */
const SHOW_COOLDOWN_MS = 30 * 24 * 60 * 60 * 1000;
const FAILED_RETRY_MS = 24 * 60 * 60 * 1000;

export interface PodcastExclusions {
  /** Every episode ever recommended (except failed rows past their retry window). */
  episodeIds: Set<CanonicalEpisodeId>;
  /** Shows on cooldown or excluded permanently via not-for-me feedback. */
  showIds: Set<CanonicalShowId>;
}

export const getPodcastExclusions = Effect.fn("PodcastRecommendations.getExclusions")(
  function* (now: number) {
    return computePodcastExclusions(yield* PodcastRecommendationEntity.getAll(), now);
  },
);

export function computePodcastExclusions(
  records: PodcastRecommendationData[],
  now: number,
): PodcastExclusions {
  const episodeIds = new Set<CanonicalEpisodeId>();
  const showIds = new Set<CanonicalShowId>();
  const latestFeedbackByShow = new Map<CanonicalShowId, PodcastRecommendationData>();

  for (const rec of records) {
    if (rec.status === PodcastRecommendationStatus.Failed) {
      if (now - rec.recommendedAt < FAILED_RETRY_MS) episodeIds.add(rec.episodeId);
    } else {
      // A delivered episode is never recommended again, regardless of outcome.
      episodeIds.add(rec.episodeId);
      if (now - rec.recommendedAt < SHOW_COOLDOWN_MS) showIds.add(rec.showId);
    }

    const prior = latestFeedbackByShow.get(rec.showId);
    if (
      rec.feedback &&
      (!prior ||
        (rec.feedbackAt ?? rec.recommendedAt) >
          (prior.feedbackAt ?? prior.recommendedAt))
    ) {
      latestFeedbackByShow.set(rec.showId, rec);
    }
  }

  // Not-for-me excludes the whole show permanently unless newer feedback
  // corrects it (latest feedback wins, mirroring the media recs system).
  for (const rec of latestFeedbackByShow.values()) {
    if (rec.feedback === "not_for_me") showIds.add(rec.showId);
  }

  return { episodeIds, showIds };
}

/** Recommendations still awaiting an outcome label. */
export const getOpenPodcastRecommendations = Effect.fn(
  "PodcastRecommendations.getOpen",
)(function* () {
  return (yield* PodcastRecommendationEntity.getAll()).filter(
    (r) =>
      (r.status === PodcastRecommendationStatus.Notified ||
        r.status === PodcastRecommendationStatus.Pending) &&
      r.feedback !== "not_for_me",
  );
});

export type PodcastFeedbackInput = {
  feedback?: PodcastFeedback;
  note?: string;
};

/**
 * Overloaded so existing callers passing a bare enum keep compiling: the note
 * is purely additive, and either input alone (or both) is a valid update.
 */
export const setPodcastRecommendationFeedback = Effect.fn(
  "PodcastRecommendations.setFeedback",
)(function* (recommendationId: string, input: PodcastFeedback | PodcastFeedbackInput) {
  const rec = Option.getOrUndefined(
    yield* PodcastRecommendationEntity.get({ recommendationId }),
  );
  if (!rec) return undefined;
  const { feedback, note } =
    typeof input === "string" ? { feedback: input, note: undefined } : input;
  const patch: Partial<Omit<PodcastRecommendationData, "recommendationId">> = {
    feedbackAt: Date.now(),
  };
  if (feedback !== undefined) patch.feedback = feedback;
  if (note !== undefined) patch.feedbackNote = note;
  yield* PodcastRecommendationEntity.patch({ recommendationId }, patch);
  return Option.getOrUndefined(
    yield* PodcastRecommendationEntity.get({ recommendationId }),
  );
});

export const formatPodcastFeedbackDigest = Effect.fn(
  "PodcastRecommendations.formatFeedbackDigest",
)(function* () {
  return formatPodcastFeedbackDigestFrom(yield* getAllPodcastRecommendations());
});

export function formatPodcastFeedbackDigestFrom(
  input: PodcastRecommendationData[],
): string {
  const seen = new Set<string>();
  const records = [...input]
    .sort(
      (a, b) => (b.feedbackAt ?? b.recommendedAt) - (a.feedbackAt ?? a.recommendedAt),
    )
    .filter((rec) => {
      if ((!rec.feedback && !rec.feedbackNote) || seen.has(rec.showId)) return false;
      seen.add(rec.showId);
      return true;
    })
    .slice(0, 30);
  if (records.length === 0) return "No explicit podcast feedback yet.";

  const good = records.filter((rec) => rec.feedback === "good_pick");
  const bad = records.filter((rec) => rec.feedback === "not_for_me");
  const noteOnly = records.filter((rec) => !rec.feedback && rec.feedbackNote);
  const lines = ["Explicit feedback on past podcast recommendations:"];
  if (good.length > 0) {
    const picks = good.map((r) => formatPodcastFeedbackEntry(r, true)).join("; ");
    lines.push(`- Good picks: ${picks}`);
  }
  if (bad.length > 0) {
    const skips = bad.map((r) => formatPodcastFeedbackEntry(r, false)).join("; ");
    lines.push(`- Not for me: ${skips}`);
  }
  if (noteOnly.length > 0) {
    const notes = noteOnly.map((r) => formatPodcastFeedbackEntry(r, true)).join("; ");
    lines.push(`- Notes (no rating): ${notes}`);
  }
  return lines.join("\n");
}

function formatPodcastFeedbackEntry(
  rec: PodcastRecommendationData,
  includeEpisode: boolean,
): string {
  const base = includeEpisode
    ? `${rec.showTitle} — ${rec.episodeTitle}`
    : rec.showTitle;
  return rec.feedbackNote ? `${base} — note: "${rec.feedbackNote}"` : base;
}

/** Recently recommended episodes, for the discovery/selection dedup context. */
export const formatRecentRecommendationsDigest = Effect.fn(
  "PodcastRecommendations.formatRecentDigest",
)(function* (limit = 15) {
  const recent = (yield* getAllPodcastRecommendations()).slice(0, limit);
  if (recent.length === 0) return "No podcast episodes recommended yet.";
  return [
    "Recently recommended episodes (never repeat these):",
    ...recent.map(
      (r) => `- ${r.showTitle} — ${r.episodeTitle} (${toDateStamp(r.recommendedAt)})`,
    ),
  ].join("\n");
});

/**
 * Persisted cursor for rotating through the voices list. Person-searching every
 * voice each run would be costly, so we cover a bounded batch per run and pick
 * up where we left off next time.
 */
type PodcastRunStateData = { id: "singleton"; voiceCursor: number };

const PodcastRunStateEntity = new Entity<PodcastRunStateData, ["id"]>(
  "podcast-run-state",
  ["id"],
);

/** Pure rotation core: the batch to search now and the cursor to store next. */
export function computeVoiceBatch(
  voices: string[],
  max: number,
  cursor: number,
): { batch: string[]; nextCursor: number } {
  if (voices.length === 0 || max <= 0) return { batch: [], nextCursor: cursor };
  if (voices.length <= max) return { batch: [...voices], nextCursor: 0 };
  const start = ((cursor % voices.length) + voices.length) % voices.length;
  const batch: string[] = [];
  for (let i = 0; i < max; i++) {
    batch.push(voices[(start + i) % voices.length] as string);
  }
  return { batch, nextCursor: (start + max) % voices.length };
}

/**
 * Returns the next batch of up to `max` voices to person-search this run.
 * The caller must explicitly commit the cursor after discovery succeeds. This
 * prevents a transient discovery outage from silently skipping a whole batch.
 */
export const nextVoiceBatch = Effect.fn("PodcastRecommendations.nextVoiceBatch")(
  function* (voices: string[], max: number) {
    const cursor =
      Option.getOrUndefined(yield* PodcastRunStateEntity.get({ id: "singleton" }))
        ?.voiceCursor ?? 0;
    return computeVoiceBatch(voices, max, cursor).batch;
  },
);

/** Advance the voice rotation only after the selected batch was searched. */
export const advanceVoiceCursor = Effect.fn(
  "PodcastRecommendations.advanceVoiceCursor",
)(function* (voices: string[], max: number) {
  const cursor =
    Option.getOrUndefined(yield* PodcastRunStateEntity.get({ id: "singleton" }))
      ?.voiceCursor ?? 0;
  const { nextCursor } = computeVoiceBatch(voices, max, cursor);
  if (voices.length > max) {
    yield* PodcastRunStateEntity.upsert({ id: "singleton", voiceCursor: nextCursor });
  }
});
