import { useEffect, useMemo, useState } from "react";
import { fetchRecommendations } from "../api";
import type {
  Recommendation,
  RecommendationStatus,
  WatchlistResult,
} from "../api";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";
import { formatDateOnly } from "../utils/format";

const TASK_NAME = "Recommendations";

const STATUS_LABELS: Record<RecommendationStatus, string> = {
  pending: "pending",
  notified: "notified",
  watched: "watched",
  abandoned: "abandoned",
  ignored: "ignored",
  failed: "failed",
};

const STATUS_ORDER: RecommendationStatus[] = [
  "notified",
  "pending",
  "watched",
  "abandoned",
  "ignored",
  "failed",
];

const WATCHLIST_LABELS: Record<WatchlistResult, string> = {
  added: "added to watchlist",
  already_exists: "already on watchlist",
  skipped: "watchlist skipped",
  error: "watchlist error",
};

function Poster({ rec }: { rec: Recommendation }) {
  const [broken, setBroken] = useState(false);

  if (!rec.posterPath || broken) {
    return (
      <div className="rec-poster rec-poster-placeholder">
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
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="M3 9h18M7 4v5M12 4v5M17 4v5" />
        </svg>
      </div>
    );
  }

  return (
    <img
      className="rec-poster"
      src={`https://image.tmdb.org/t/p/w185${rec.posterPath}`}
      alt={`${rec.title} poster`}
      loading="lazy"
      onError={() => setBroken(true)}
    />
  );
}

function RecommendationCard({ rec }: { rec: Recommendation }) {
  return (
    <div className="rec-card">
      <Poster rec={rec} />
      <div className="rec-body">
        <div className="rec-title-row">
          <span className="rec-title">
            {rec.title}
            {rec.year !== null && <span className="rec-year"> ({rec.year})</span>}
          </span>
          <span className={`media-badge media-${rec.mediaType}`}>
            {rec.mediaType === "tv" ? "TV" : "Movie"}
          </span>
          <span className={`status-chip status-chip-${rec.status}`}>
            {STATUS_LABELS[rec.status]}
          </span>
          {rec.watchlistResult && (
            <span className={`watchlist-badge watchlist-${rec.watchlistResult}`}>
              {WATCHLIST_LABELS[rec.watchlistResult]}
            </span>
          )}
        </div>
        {rec.whyForUser && <p className="rec-why">{rec.whyForUser}</p>}
        {rec.caveats.length > 0 && (
          <ul className="rec-caveats">
            {rec.caveats.map((caveat) => (
              <li key={caveat}>{caveat}</li>
            ))}
          </ul>
        )}
        <div className="rec-meta">
          <span>Recommended {formatDateOnly(rec.recommendedAt)}</span>
          {rec.confidence !== null && (
            <span className="muted">
              &middot; confidence {Math.round(rec.confidence * 100)}%
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

export default function RecommendationsPage() {
  const [recs, setRecs] = useState<Recommendation[] | null>(null);
  const [recsError, setRecsError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<RecommendationStatus | "">("");
  const { snapshot, runTask } = useLiveData();
  const { toast, showToast } = useToast();

  const recTask = useMemo(
    () => snapshot?.tasks.find((t) => t.name === TASK_NAME) ?? null,
    [snapshot],
  );
  const running = recTask?.running ?? false;
  // Once the snapshot has loaded, a missing Recommendations task means it's
  // disabled server-side (missing API keys) — don't offer a doomed Run button.
  const taskAvailable = snapshot === null || recTask !== null;

  // Load once, then reload whenever the Recommendations task finishes running
  // so freshly generated picks appear without a manual refresh.
  useEffect(() => {
    if (running) return;
    let cancelled = false;
    fetchRecommendations()
      .then((data) => {
        if (cancelled) return;
        setRecs(data.recommendations);
        setRecsError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setRecsError(
          err instanceof Error ? err.message : "Failed to fetch recommendations",
        );
      });
    return () => {
      cancelled = true;
    };
  }, [running]);

  const handleRun = async () => {
    const result = await runTask(TASK_NAME);
    showToast(result.message, result.ok ? "info" : "error");
  };

  const statusCounts = useMemo(() => {
    const counts = new Map<RecommendationStatus, number>();
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
        <h1>Recommendations</h1>
        <button
          type="button"
          className="run-btn"
          disabled={running || !taskAvailable}
          title={
            taskAvailable
              ? undefined
              : "Task disabled: missing TMDB/OpenAI/Tavily API keys"
          }
          onClick={handleRun}
        >
          {running ? (
            <>
              <span className="running-pulse" /> Running…
            </>
          ) : taskAvailable ? (
            "Run now"
          ) : (
            "Task disabled"
          )}
        </button>
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
          <div>Failed to load recommendations</div>
          <div className="error-detail">{recsError}</div>
        </div>
      )}
      {recs !== null && recs.length === 0 && (
        <div className="rec-empty">
          <div className="rec-empty-title">No recommendations yet</div>
          <div className="muted">
            The Recommendations task hasn&apos;t produced any picks. Hit
            &ldquo;Run now&rdquo; to generate the first batch.
          </div>
        </div>
      )}
      {visible !== null && visible.length > 0 && (
        <div className="rec-list">
          {visible.map((rec) => (
            <RecommendationCard key={rec.canonicalId} rec={rec} />
          ))}
        </div>
      )}
    </>
  );
}
