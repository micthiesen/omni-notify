import { useEffect, useMemo, useState } from "react";
import {
  fetchPodcastRecommendations,
  fetchPodcastTasteProfile,
  sendPodcastRecommendationFeedback,
} from "../api";
import type {
  PodcastFeedback,
  PodcastRecommendation,
  PodcastRecommendationStatus,
  PodcastTasteProfile,
} from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { StatusFilterChips } from "../components/StatusFilterChips";
import { TasteBrain } from "../components/TasteBrain";
import { Toast, useToast } from "../components/Toast";
import { useRecHighlight } from "../hooks/useRecHighlight";
import { useLiveData } from "../live";
import { formatAbsolute, formatDateOnly, formatRelative } from "../utils/format";

const TASTE_TASK_NAME = "PodcastTasteReflection";

const STATUS_LABELS: Record<PodcastRecommendationStatus, string> = {
  pending: "Pending",
  notified: "Notified",
  listened: "Listened",
  abandoned: "Abandoned",
  ignored: "Ignored",
  failed: "Failed",
};

const STATUS_ORDER: PodcastRecommendationStatus[] = [
  "notified",
  "pending",
  "listened",
  "abandoned",
  "ignored",
  "failed",
];

const FEEDBACK_ACTIONS: { value: PodcastFeedback; label: string }[] = [
  { value: "good_pick", label: "Good pick" },
  { value: "not_for_me", label: "Not for me" },
];

function formatEpisodeDuration(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = Math.round(minutes % 60);
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`;
  return `${m}m`;
}

function Artwork({ rec }: { rec: PodcastRecommendation }) {
  return (
    <ImageWithFallback
      src={rec.artworkUrl ?? null}
      alt={`${rec.showTitle} artwork`}
      className="podrec-artwork"
      placeholderClassName="podrec-artwork-placeholder"
      loading="lazy"
      placeholder={
        <svg
          width="28"
          height="28"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="9" y="2" width="6" height="12" rx="3" />
          <path d="M5 10a7 7 0 0 0 14 0" />
          <path d="M12 17v4M9 21h6" />
        </svg>
      }
    />
  );
}

function PodcastCard({
  rec,
  saving,
  highlighted,
  onFeedback,
}: {
  rec: PodcastRecommendation;
  saving: boolean;
  highlighted: boolean;
  onFeedback: (feedback: PodcastFeedback) => void;
}) {
  const canRate = rec.status !== "pending" && rec.status !== "failed";
  return (
    <div
      id={`recommendation-${rec.recommendationId}`}
      className={`rec-card ${highlighted ? "rec-card-highlighted" : ""}`}
    >
      <Artwork rec={rec} />
      <div className="rec-body">
        <div className="rec-title-row">
          <span className="rec-title">{rec.episodeTitle}</span>
          <span className={`status-chip status-chip-${rec.status}`}>
            {STATUS_LABELS[rec.status]}
          </span>
        </div>
        <div className="podrec-show">{rec.showTitle}</div>
        {rec.matchedVoices && rec.matchedVoices.length > 0 && (
          <div className="podrec-featuring">
            🎙️ Featuring {rec.matchedVoices.join(", ")}
          </div>
        )}
        {rec.whyForUser && <p className="rec-why">{rec.whyForUser}</p>}
        {rec.caveats && rec.caveats.length > 0 && (
          <ul className="rec-caveats">
            {rec.caveats.map((caveat) => (
              <li key={caveat}>{caveat}</li>
            ))}
          </ul>
        )}
        <div className="rec-meta meta-row">
          <span>Released {formatDateOnly(rec.publishedAt)}</span>
          {rec.durationMinutes != null && (
            <span className="muted">
              {formatEpisodeDuration(rec.durationMinutes)}
            </span>
          )}
          <span className="muted" title={formatAbsolute(rec.recommendedAt)}>
            Recommended {formatRelative(rec.recommendedAt)}
          </span>
          {(rec.queueResult === "queued" || rec.queueResult === "already_queued") && (
            <span
              className="podrec-queued"
              title="This episode is waiting in your Castro queue"
            >
              🎧 In Castro queue
            </span>
          )}
        </div>
        {(rec.episodeUrl || rec.sourceUrl) && (
          <div className="rec-links">
            {rec.episodeUrl && (
              <a href={rec.episodeUrl} target="_blank" rel="noreferrer">
                Episode page
              </a>
            )}
            {rec.sourceUrl && (
              <a href={rec.sourceUrl} target="_blank" rel="noreferrer">
                Discussion
              </a>
            )}
          </div>
        )}
        {canRate && (
          <div className="rec-feedback" aria-label="Recommendation feedback">
            {FEEDBACK_ACTIONS.map((action) => (
              <button
                key={action.value}
                type="button"
                className={`feedback-btn ${rec.feedback === action.value ? "active" : ""}`}
                aria-pressed={rec.feedback === action.value}
                disabled={saving}
                onClick={() => onFeedback(action.value)}
              >
                {action.label}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function PodcastTasteBrain({
  profile,
  loading,
  error,
}: {
  profile: PodcastTasteProfile | null;
  loading: boolean;
  error: string | null;
}) {
  const stats: [string, React.ReactNode][] = profile
    ? [
        ["Episodes finished", profile.stats.listenedEpisodes],
        ["Episodes started", profile.stats.startedEpisodes],
        ["Starred", profile.stats.starredEpisodes],
        ["Shows heard", profile.stats.distinctShows],
        [
          "Recommendations listened",
          `${profile.stats.recommendations.listened}/${profile.stats.recommendations.total}`,
        ],
        ["Good picks", profile.stats.feedback.goodPick],
      ]
    : [];

  return (
    <TasteBrain
      profile={profile}
      loading={loading}
      error={error}
      subtitle="Castro listening and feedback are reflected into a versioned taste profile."
      emptyText="No profile yet. The reflection task will build one from Castro listen history and recommendation feedback."
      stats={stats}
    />
  );
}

export default function PodcastsPage() {
  const [recs, setRecs] = useState<PodcastRecommendation[] | null>(null);
  const [recsError, setRecsError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<PodcastRecommendationStatus | "">(
    "",
  );
  const [savingId, setSavingId] = useState<string | null>(null);
  const [tasteProfile, setTasteProfile] = useState<PodcastTasteProfile | null>(null);
  const [tasteLoading, setTasteLoading] = useState(true);
  const [tasteError, setTasteError] = useState<string | null>(null);
  const { toast, showToast } = useToast();
  const { snapshot } = useLiveData();

  const latestTasteRunId =
    snapshot?.runs.find((run) => run.taskName === TASTE_TASK_NAME)?.runId ?? null;

  // Load once, then reload whenever a reflection run lands so a fresh profile
  // version appears without a manual refresh.
  useEffect(() => {
    let cancelled = false;
    fetchPodcastTasteProfile()
      .then((data) => {
        if (cancelled) return;
        setTasteProfile(data.profile);
        setTasteError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setTasteError(
          err instanceof Error ? err.message : "Failed to fetch taste profile",
        );
      })
      .finally(() => {
        if (!cancelled) setTasteLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [latestTasteRunId]);

  useEffect(() => {
    let cancelled = false;
    fetchPodcastRecommendations()
      .then((data) => {
        if (cancelled) return;
        setRecs(data.recommendations);
        setRecsError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setRecsError(
          err instanceof Error ? err.message : "Failed to fetch podcast recommendations",
        );
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleFeedback = async (
    recommendationId: string,
    feedback: PodcastFeedback,
  ) => {
    setSavingId(recommendationId);
    try {
      const result = await sendPodcastRecommendationFeedback(
        recommendationId,
        feedback,
      );
      setRecs((current) =>
        current?.map((rec) =>
          rec.recommendationId === recommendationId ? result.recommendation : rec,
        ) ?? null,
      );
      showToast("Feedback saved", "info");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Failed to save feedback", "error");
    } finally {
      setSavingId(null);
    }
  };

  const highlightedId = useRecHighlight(recs !== null);

  const statusCounts = useMemo(() => {
    const counts = new Map<PodcastRecommendationStatus, number>();
    for (const rec of recs ?? []) {
      counts.set(rec.status, (counts.get(rec.status) ?? 0) + 1);
    }
    return counts;
  }, [recs]);

  const visible = useMemo(() => {
    if (recs === null) return null;
    return statusFilter === ""
      ? recs
      : recs.filter((r) => r.status === statusFilter);
  }, [recs, statusFilter]);

  return (
    <>
      <div className="page-header">
        <h1>Podcasts</h1>
      </div>
      <Toast toast={toast} />

      <PodcastTasteBrain
        profile={tasteProfile}
        loading={tasteLoading}
        error={tasteError}
      />

      {recs !== null && recs.length > 0 && (
        <StatusFilterChips
          order={STATUS_ORDER}
          labels={STATUS_LABELS}
          counts={statusCounts}
          total={recs.length}
          active={statusFilter}
          onChange={setStatusFilter}
        />
      )}

      {recs === null && recsError === null && <div className="loading">Loading…</div>}
      {recsError && recs === null && (
        <div className="error">
          <div>Failed to load podcast recommendations</div>
          <div className="error-detail">{recsError}</div>
        </div>
      )}
      {recs !== null && recs.length === 0 && (
        <div className="rec-empty">
          <div className="rec-empty-title">No podcast recommendations yet</div>
          <div className="muted">
            The podcast recommendation task hasn’t produced any picks yet.
          </div>
        </div>
      )}
      {visible !== null && visible.length > 0 && (
        <div className="rec-list">
          {visible.map((rec) => (
            <PodcastCard
              key={rec.recommendationId}
              rec={rec}
              saving={savingId === rec.recommendationId}
              highlighted={highlightedId === rec.recommendationId}
              onFeedback={(feedback) =>
                void handleFeedback(rec.recommendationId, feedback)
              }
            />
          ))}
        </div>
      )}
    </>
  );
}
