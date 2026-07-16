import { useEffect, useMemo, useState } from "react";
import {
  fetchPodcastRecommendations,
  sendPodcastRecommendationFeedback,
} from "../api";
import type {
  PodcastFeedback,
  PodcastRecommendation,
  PodcastRecommendationStatus,
} from "../api";
import { Toast, useToast } from "../components/Toast";
import { formatAbsolute, formatDateOnly, formatRelative } from "../utils/format";

const STATUS_LABELS: Record<PodcastRecommendationStatus, string> = {
  pending: "pending",
  notified: "notified",
  listened: "listened",
  abandoned: "abandoned",
  ignored: "ignored",
  failed: "failed",
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
  const [broken, setBroken] = useState(false);

  if (!rec.artworkUrl || broken) {
    return (
      <div className="podrec-artwork podrec-artwork-placeholder">
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
      </div>
    );
  }

  return (
    <img
      className="podrec-artwork"
      src={rec.artworkUrl}
      alt={`${rec.showTitle} artwork`}
      loading="lazy"
      onError={() => setBroken(true)}
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
        {rec.whyForUser && <p className="rec-why">{rec.whyForUser}</p>}
        {rec.caveats && rec.caveats.length > 0 && (
          <ul className="rec-caveats">
            {rec.caveats.map((caveat) => (
              <li key={caveat}>{caveat}</li>
            ))}
          </ul>
        )}
        <div className="rec-meta">
          <span>Released {formatDateOnly(rec.publishedAt)}</span>
          {rec.durationMinutes != null && (
            <span className="muted">
              &middot; {formatEpisodeDuration(rec.durationMinutes)}
            </span>
          )}
          <span className="muted" title={formatAbsolute(rec.recommendedAt)}>
            &middot; recommended {formatRelative(rec.recommendedAt)}
          </span>
          {(rec.queueResult === "queued" || rec.queueResult === "already_queued") && (
            <span
              className="podrec-queued"
              title="This episode is waiting in your Castro queue"
            >
              &middot; 🎧 in Castro queue
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

export default function PodcastsPage() {
  const [recs, setRecs] = useState<PodcastRecommendation[] | null>(null);
  const [recsError, setRecsError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<PodcastRecommendationStatus | "">(
    "",
  );
  const [savingId, setSavingId] = useState<string | null>(null);
  const { toast, showToast } = useToast();

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

  const highlightedId = new URLSearchParams(window.location.search).get(
    "recommendation",
  );

  useEffect(() => {
    if (!highlightedId || recs === null) return;
    document
      .getElementById(`recommendation-${highlightedId}`)
      ?.scrollIntoView({ behavior: "smooth", block: "center" });
  }, [highlightedId, recs]);

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

      {recs !== null && recs.length > 0 && (
        <div className="rec-filters">
          <button
            type="button"
            className={`chip-btn ${statusFilter === "" ? "active" : ""}`}
            onClick={() => setStatusFilter("")}
          >
            All ({recs.length})
          </button>
          {STATUS_ORDER.filter((s) => statusCounts.has(s)).map((status) => (
            <button
              key={status}
              type="button"
              className={`chip-btn ${statusFilter === status ? "active" : ""}`}
              onClick={() =>
                setStatusFilter((prev) => (prev === status ? "" : status))
              }
            >
              {STATUS_LABELS[status]} ({statusCounts.get(status)})
            </button>
          ))}
        </div>
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
