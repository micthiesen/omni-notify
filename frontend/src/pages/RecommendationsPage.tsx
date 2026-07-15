import { useEffect, useMemo, useState } from "react";
import {
  fetchRecommendations,
  fetchTaskRuns,
  fetchTasteProfile,
  sendRecommendationFeedback,
} from "../api";
import type {
  Recommendation,
  RecommendationFeedback,
  RecommendationStatus,
  TaskRun,
  TasteClaim,
  TasteProfile,
  WatchlistResult,
} from "../api";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";
import { formatAbsolute, formatDateOnly, formatRelative } from "../utils/format";

const TASK_NAME = "Recommendations";
const TASTE_TASK_NAME = "TasteReflection";

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
  available: "available in Plex",
  error: "watchlist error",
};

const FEEDBACK_ACTIONS: { value: RecommendationFeedback; label: string }[] = [
  { value: "good_pick", label: "Good pick" },
  { value: "not_for_me", label: "Not for me" },
  { value: "already_watched", label: "Already watched" },
];

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

function ClaimList({ claims }: { claims: TasteClaim[] }) {
  return (
    <ul className="taste-claim-list">
      {claims.map((item) => (
        <li key={item.claim}>
          <span>{item.claim}</span>
          <span
            className="taste-confidence"
            title={`${item.evidenceIds.length} supporting evidence item${item.evidenceIds.length === 1 ? "" : "s"}`}
          >
            {Math.round(item.confidence * 100)}%
          </span>
        </li>
      ))}
    </ul>
  );
}

function TasteBrain({
  profile,
  loading,
  error,
}: {
  profile: TasteProfile | null;
  loading: boolean;
  error: string | null;
}) {
  const stats = profile
    ? [
        ["Completed movies", profile.stats.completedMovies],
        ["Completed series", profile.stats.completedSeries],
        ["Rewatched titles", profile.stats.rewatchedTitles],
        [
          "Recommendations watched",
          `${profile.stats.recommendations.watched}/${profile.stats.recommendations.total}`,
        ],
        ["Good picks", profile.stats.feedback.goodPick],
        [
          "Average time to start",
          profile.stats.averageHoursToStart === undefined
            ? "Not enough data"
            : `${profile.stats.averageHoursToStart.toFixed(1)}h`,
        ],
      ]
    : [];

  return (
    <section className="page-section taste-brain">
      <div className="taste-heading">
        <div>
          <h2 className="section-title">Taste brain</h2>
          <div className="muted taste-subtitle">
            Watching and feedback are reflected into a versioned taste profile.
          </div>
        </div>
        {profile && (
          <span
            className="taste-version"
            title={formatAbsolute(profile.generatedAt)}
          >
            v{profile.version} · {formatRelative(profile.generatedAt)}
          </span>
        )}
      </div>

      {loading && <div className="loading-inline">Loading taste profile…</div>}
      {!loading && error && (
        <div className="error-inline">Taste profile unavailable: {error}</div>
      )}
      {!loading && !error && profile === null && (
        <div className="taste-empty">
          No profile yet. The reflection task will build one from Plex watching and
          recommendation feedback.
        </div>
      )}
      {profile && (
        <div className="taste-card">
          <p className="taste-summary">{profile.summary}</p>
          {stats.length > 0 && (
            <div className="taste-stats">
              {stats.map(([name, value]) => (
                <div className="taste-stat" key={String(name)}>
                  <span>{name}</span>
                  <strong>{value}</strong>
                </div>
              ))}
            </div>
          )}
          <div className="taste-columns">
            {profile.stablePreferences.length > 0 && (
              <div>
                <h3>Reliable preferences</h3>
                <ClaimList claims={profile.stablePreferences} />
              </div>
            )}
            {profile.conditionalPreferences.length > 0 && (
              <div>
                <h3>Depends on context</h3>
                <ClaimList claims={profile.conditionalPreferences} />
              </div>
            )}
            {profile.aversions.length > 0 && (
              <div>
                <h3>Avoid</h3>
                <ClaimList claims={profile.aversions} />
              </div>
            )}
            {profile.uncertainties.length > 0 && (
              <div>
                <h3>Still learning</h3>
                <ClaimList claims={profile.uncertainties} />
              </div>
            )}
          </div>
          {(profile.explorationTargets.length > 0 ||
            profile.currentSaturation.length > 0) && (
            <div className="taste-tags-row">
              {profile.explorationTargets.length > 0 && (
                <div>
                  <span className="taste-tags-label">Explore</span>
                  <div className="taste-tags">
                    {profile.explorationTargets.map((target) => (
                      <span
                        className="taste-tag taste-tag-explore"
                        key={target.claim}
                        title={`${target.evidenceIds.length} supporting evidence item(s)`}
                      >
                        {target.claim}
                      </span>
                    ))}
                  </div>
                </div>
              )}
              {profile.currentSaturation.length > 0 && (
                <div>
                  <span className="taste-tags-label">Currently saturated</span>
                  <div className="taste-tags">
                    {profile.currentSaturation.map((target) => (
                      <span
                        className="taste-tag"
                        key={target.claim}
                        title={`${target.evidenceIds.length} supporting evidence item(s)`}
                      >
                        {target.claim}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
          <div className="taste-commitments">
            <span>Commitment fit</span>
            <span>Movies: {profile.commitmentPreferences.movies.preference}</span>
            <span>
              Limited series: {profile.commitmentPreferences.limitedSeries.preference}
            </span>
            <span>
              Long series: {profile.commitmentPreferences.longSeries.preference}
            </span>
          </div>
        </div>
      )}
    </section>
  );
}

function runOutcome(run: TaskRun): { label: string; tone: string } {
  if (run.status === "running") return { label: "Running", tone: "running" };
  if (run.status === "error") return { label: "Error", tone: "error" };
  if (run.summary?.startsWith("no_add:")) {
    return { label: "No pick", tone: "no-add" };
  }
  return { label: "Completed", tone: "success" };
}

function RecommendationActivity({ runs }: { runs: TaskRun[] }) {
  return (
    <section className="page-section rec-activity-section">
      <h2 className="section-title">Recent recommendation runs</h2>
      {runs.length === 0 ? (
        <div className="muted">No recommendation runs recorded yet.</div>
      ) : (
        <div className="rec-run-list">
          {runs.map((run) => {
            const outcome = runOutcome(run);
            return (
              <div className="rec-run-row" key={run.runId}>
                <span className={`rec-run-outcome rec-run-${outcome.tone}`}>
                  {outcome.label}
                </span>
                <span
                  className="rec-run-time"
                  title={formatAbsolute(run.startedAt)}
                >
                  {formatRelative(run.startedAt)}
                </span>
                <span className={run.error ? "run-error" : "run-summary"}>
                  {run.error ?? run.summary ?? "Run completed without a summary"}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </section>
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
  const [recommendationRuns, setRecommendationRuns] = useState<TaskRun[]>([]);
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

  useEffect(() => {
    let cancelled = false;
    fetchTaskRuns({ task: TASK_NAME, limit: 6 })
      .then((data) => {
        if (!cancelled) setRecommendationRuns(data.runs);
      })
      .catch(() => {
        // Recommendation cards remain useful if activity history is unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, [latestRecommendationRunId]);

  const handleRun = async () => {
    const result = await runTask(TASK_NAME);
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

      <TasteBrain
        profile={tasteProfile}
        loading={tasteLoading}
        error={tasteError}
      />

      <RecommendationActivity runs={recommendationRuns} />

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
            {taskAvailable
              ? "The Recommendations task hasn’t produced any picks. Run it to generate the first one."
              : "Add the required recommendation service credentials to enable the first run."}
          </div>
        </div>
      )}
      {visible !== null && visible.length > 0 && (
        <div className="rec-list">
          {visible.map((rec) => (
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
      )}
    </>
  );
}
