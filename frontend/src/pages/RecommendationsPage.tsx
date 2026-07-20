import { useEffect, useMemo, useState } from "react";
import {
  fetchRecommendations,
  fetchTasteProfile,
  sendRecommendationFeedback,
} from "../api";
import type {
  Recommendation,
  RecommendationFeedback,
  RecommendationStatus,
  TasteProfile,
} from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { RecommendationRuns } from "../components/RecommendationRuns";
import { ShowMoreButton, useShowMore } from "../components/ShowMore";
import { StatusFilterChips } from "../components/StatusFilterChips";
import { TasteBrain } from "../components/TasteBrain";
import { Toast, useToast } from "../components/Toast";
import { useRecHighlight } from "../hooks/useRecHighlight";
import { useLiveData } from "../live";
import { Link } from "../router";
import { formatDateOnly } from "../utils/format";
import {
  REC_FEEDBACK_ACTIONS as FEEDBACK_ACTIONS,
  REC_STATUS_LABELS as STATUS_LABELS,
  REC_STATUS_ORDER as STATUS_ORDER,
  WATCHLIST_LABELS,
} from "../utils/recLabels";

const TASK_NAME = "Recommendations";
const TASTE_TASK_NAME = "TasteReflection";

function Poster({ rec }: { rec: Recommendation }) {
  return (
    <ImageWithFallback
      src={rec.posterPath ? `https://image.tmdb.org/t/p/w185${rec.posterPath}` : null}
      alt={`${rec.title} poster`}
      className="rec-poster"
      placeholderClassName="rec-poster-placeholder"
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
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="M3 9h18M7 4v5M12 4v5M17 4v5" />
        </svg>
      }
    />
  );
}

function RecommendationCard({
  rec,
  saving,
  highlighted,
  onFeedback,
}: {
  rec: Recommendation;
  saving: boolean;
  highlighted: boolean;
  onFeedback: (feedback: RecommendationFeedback) => void;
}) {
  const canRate = rec.status !== "pending" && rec.status !== "failed";
  return (
    <div
      id={`recommendation-${rec.recommendationId}`}
      className={`rec-card ${highlighted ? "rec-card-highlighted" : ""}`}
    >
      <Poster rec={rec} />
      <div className="rec-body">
        <div className="rec-title-row">
          <Link
            to={`/media/${encodeURIComponent(rec.recommendationId)}`}
            className="rec-title rec-title-link"
            title="View details"
          >
            {rec.title}
            {rec.year !== null && <span className="rec-year"> ({rec.year})</span>}
          </Link>
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
        <div className="rec-meta meta-row">
          <span>Recommended {formatDateOnly(rec.recommendedAt)}</span>
          {rec.confidence !== null && (
            <span className="muted">
              Confidence {Math.round(rec.confidence * 100)}%
            </span>
          )}
        </div>
        <div className="rec-links">
          <a href={rec.links.tmdb} target="_blank" rel="noreferrer">
            TMDB
          </a>
          <a href={rec.links.plex} target="_blank" rel="noreferrer">
            Plex
          </a>
          <a href={rec.links.manager} target="_blank" rel="noreferrer">
            {rec.mediaType === "movie" ? "Radarr" : "Sonarr"}
          </a>
        </div>
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

function MediaTasteBrain({
  profile,
  loading,
  error,
}: {
  profile: TasteProfile | null;
  loading: boolean;
  error: string | null;
}) {
  const stats: [string, React.ReactNode][] = profile
    ? [
        ["Completed Movies", profile.stats.completedMovies],
        ["Completed Series", profile.stats.completedSeries],
        ["Rewatched Titles", profile.stats.rewatchedTitles],
        [
          "Recommendations Watched",
          `${profile.stats.recommendations.watched}/${profile.stats.recommendations.total}`,
        ],
        ["Good Picks", profile.stats.feedback.goodPick],
        [
          "Average Time to Start",
          profile.stats.averageHoursToStart === undefined
            ? "Not enough data"
            : `${profile.stats.averageHoursToStart.toFixed(1)}h`,
        ],
      ]
    : [];

  return (
    <TasteBrain
      profile={profile}
      loading={loading}
      error={error}
      subtitle="Watching and feedback are reflected into a versioned taste profile."
      emptyText="No profile yet. The reflection task will build one from Plex watching and recommendation feedback."
      stats={stats}
      footer={
        profile && (
          <div className="taste-commitments">
            <span>Commitment Fit</span>
            <span>Movies: {profile.commitmentPreferences.movies.preference}</span>
            <span>
              Limited Series: {profile.commitmentPreferences.limitedSeries.preference}
            </span>
            <span>
              Long Series: {profile.commitmentPreferences.longSeries.preference}
            </span>
          </div>
        )
      }
    />
  );
}

export default function RecommendationsPage() {
  const [recs, setRecs] = useState<Recommendation[] | null>(null);
  const [recsError, setRecsError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<RecommendationStatus | "">("");
  const [savingId, setSavingId] = useState<string | null>(null);
  const [tasteProfile, setTasteProfile] = useState<TasteProfile | null>(null);
  const [tasteLoading, setTasteLoading] = useState(true);
  const [tasteError, setTasteError] = useState<string | null>(null);
  const [maxRecommendations, setMaxRecommendations] = useState(1);
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
  const latestTasteRunId =
    snapshot?.runs.find((run) => run.taskName === TASTE_TASK_NAME)?.runId ?? null;
  const latestRecommendationRunId =
    snapshot?.runs.find((run) => run.taskName === TASK_NAME)?.runId ?? null;

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

  useEffect(() => {
    let cancelled = false;
    fetchTasteProfile()
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

  const handleRun = async () => {
    const result = await runTask(TASK_NAME, { maxRecommendations });
    showToast(result.message, result.ok ? "info" : "error");
  };

  const handleFeedback = async (
    recommendationId: string,
    feedback: RecommendationFeedback,
  ) => {
    setSavingId(recommendationId);
    try {
      const result = await sendRecommendationFeedback(recommendationId, feedback);
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

  const {
    visible: shown,
    hasMore,
    remaining,
    showMore,
  } = useShowMore(visible ?? [], 20, statusFilter);

  return (
    <>
      <div className="page-header">
        <h1>Media</h1>
        <div className="rec-run-controls">
          <label className="rec-run-limit">
            <span>Up To</span>
            <select
              aria-label="Maximum recommendations"
              value={maxRecommendations}
              disabled={running || !taskAvailable}
              onChange={(event) => setMaxRecommendations(Number(event.target.value))}
            >
              {Array.from({ length: 10 }, (_, index) => index + 1).map((count) => (
                <option key={count} value={count}>
                  {count}
                </option>
              ))}
            </select>
          </label>
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
              "Run Now"
            ) : (
              "Task Disabled"
            )}
          </button>
        </div>
      </div>
      <Toast toast={toast} />

      <MediaTasteBrain
        profile={tasteProfile}
        loading={tasteLoading}
        error={tasteError}
      />

      <RecommendationRuns
        taskName={TASK_NAME}
        latestRunId={latestRecommendationRunId}
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
          <div>Failed to load recommendations</div>
          <div className="error-detail">{recsError}</div>
        </div>
      )}
      {recs !== null && recs.length === 0 && (
        <div className="rec-empty">
          <div className="rec-empty-title">No recommendations yet</div>
          <div className="muted">
            {taskAvailable
              ? "The Recommendations task hasn’t produced any picks. Run it to generate the first one."
              : "Add the required recommendation service credentials to enable the first run."}
          </div>
        </div>
      )}
      {visible !== null && visible.length > 0 && (
        <>
          <div className="rec-list">
            {shown.map((rec) => (
              <RecommendationCard
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
          {hasMore && <ShowMoreButton remaining={remaining} onClick={showMore} />}
        </>
      )}
    </>
  );
}
