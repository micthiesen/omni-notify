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
  TasteClaim,
} from "../api";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";
import { formatAbsolute, formatDateOnly, formatRelative } from "../utils/format";

const TASTE_TASK_NAME = "PodcastTasteReflection";

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
        {rec.matchedVoices && rec.matchedVoices.length > 0 && (
          <div className="podrec-featuring">
            🎙️ featuring {rec.matchedVoices.join(", ")}
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

function TasteClaimList({ claims }: { claims: TasteClaim[] }) {
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

function PodcastTasteBrain({
  profile,
  loading,
  error,
}: {
  profile: PodcastTasteProfile | null;
  loading: boolean;
  error: string | null;
}) {
  const stats = profile
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
    <section className="page-section taste-brain">
      <div className="taste-heading">
        <div>
          <h2 className="section-title">Taste brain</h2>
          <div className="muted taste-subtitle">
            Castro listening and feedback are reflected into a versioned taste
            profile.
          </div>
        </div>
        {profile && (
          <span className="taste-version" title={formatAbsolute(profile.generatedAt)}>
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
          No profile yet. The reflection task will build one from Castro listen
          history and recommendation feedback.
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
                <TasteClaimList claims={profile.stablePreferences} />
              </div>
            )}
            {profile.conditionalPreferences.length > 0 && (
              <div>
                <h3>Depends on context</h3>
                <TasteClaimList claims={profile.conditionalPreferences} />
              </div>
            )}
            {profile.aversions.length > 0 && (
              <div>
                <h3>Avoid</h3>
                <TasteClaimList claims={profile.aversions} />
              </div>
            )}
            {profile.uncertainties.length > 0 && (
              <div>
                <h3>Still learning</h3>
                <TasteClaimList claims={profile.uncertainties} />
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
        </div>
      )}
    </section>
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

      <PodcastTasteBrain
        profile={tasteProfile}
        loading={tasteLoading}
        error={tasteError}
      />
    </>
  );
}
