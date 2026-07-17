import { useEffect, useState } from "react";
import { fetchRecommendation, sendRecommendationFeedback } from "../api";
import type { Recommendation, RecommendationFeedback } from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { Toast, useToast } from "../components/Toast";
import { Link } from "../router";
import { formatAbsoluteWithYear } from "../utils/format";
import {
  REC_FEEDBACK_ACTIONS,
  REC_STATUS_LABELS,
  WATCHLIST_LABELS,
} from "../utils/recLabels";

function formatRuntime(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = Math.round(minutes % 60);
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`;
  return `${m}m`;
}

function DetailField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="detail-field">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function ScoreRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="score-row">
      <span className="score-label">{label}</span>
      <span className="score-bar">
        <span
          className="score-bar-fill"
          style={{ width: `${Math.min(100, Math.max(0, value))}%` }}
        />
      </span>
      <span className="score-value">{Math.round(value)}</span>
    </div>
  );
}

function TimelineRow({ label, at }: { label: string; at: number }) {
  return (
    <div className="timeline-row">
      <span className="timeline-label">{label}</span>
      <span className="timeline-time">{formatAbsoluteWithYear(at)}</span>
    </div>
  );
}

export default function RecommendationDetailPage({ id }: { id: string }) {
  const [rec, setRec] = useState<Recommendation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const { toast, showToast } = useToast();

  useEffect(() => {
    let cancelled = false;
    fetchRecommendation(id)
      .then((res) => {
        if (!cancelled) setRec(res.recommendation);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  const handleFeedback = async (feedback: RecommendationFeedback) => {
    setSaving(true);
    try {
      const result = await sendRecommendationFeedback(id, feedback);
      setRec(result.recommendation);
      showToast("Feedback saved", "info");
    } catch (err) {
      showToast(err instanceof Error ? err.message : "Failed to save feedback", "error");
    } finally {
      setSaving(false);
    }
  };

  if (error) {
    return (
      <div className="error">
        <div>Failed to load this recommendation</div>
        <div className="error-detail">{error}</div>
      </div>
    );
  }
  if (!rec) return <div className="loading">Loading…</div>;

  const canRate = rec.status !== "pending" && rec.status !== "failed";
  const scores = rec.shortlistScores;

  return (
    <>
      <Link to="/media" className="detail-back">
        <span className="detail-back-arrow" aria-hidden="true">
          ←
        </span>
        All media picks
      </Link>
      <Toast toast={toast} />

      <div className="detail-head">
        <ImageWithFallback
          src={
            rec.posterPath ? `https://image.tmdb.org/t/p/w342${rec.posterPath}` : null
          }
          alt={`${rec.title} poster`}
          className="detail-art"
          placeholderClassName="detail-art-placeholder"
          placeholder="🎬"
        />
        <div className="detail-head-body">
          <h1 className="detail-title">
            {rec.title}
            {rec.year !== null && <span className="rec-year"> ({rec.year})</span>}
          </h1>
          <div className="detail-badges">
            <span className={`media-badge media-${rec.mediaType}`}>
              {rec.mediaType === "tv" ? "TV" : "Movie"}
            </span>
            <span className={`status-chip status-chip-${rec.status}`}>
              {REC_STATUS_LABELS[rec.status]}
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
              {REC_FEEDBACK_ACTIONS.map((action) => (
                <button
                  key={action.value}
                  type="button"
                  className={`feedback-btn ${rec.feedback === action.value ? "active" : ""}`}
                  aria-pressed={rec.feedback === action.value}
                  disabled={saving}
                  onClick={() => void handleFeedback(action.value)}
                >
                  {action.label}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="detail-sections">
        <section className="page-section">
          <h2 className="section-title">Details</h2>
          <dl className="detail-grid">
            {rec.genres.length > 0 && (
              <DetailField label="Genres">{rec.genres.join(", ")}</DetailField>
            )}
            {rec.runtimeMinutes !== null && (
              <DetailField label="Runtime">
                {formatRuntime(rec.runtimeMinutes)}
              </DetailField>
            )}
            {rec.seasonCount !== null && (
              <DetailField label="Seasons">
                {rec.seasonCount}
                {rec.episodeCount !== null && ` (${rec.episodeCount} episodes)`}
              </DetailField>
            )}
            {rec.seriesStatus && (
              <DetailField label="Series status">{rec.seriesStatus}</DetailField>
            )}
            {rec.certification && (
              <DetailField label="Rated">{rec.certification}</DetailField>
            )}
            {rec.originalLanguage && (
              <DetailField label="Language">
                {rec.originalLanguage.toUpperCase()}
              </DetailField>
            )}
            {rec.originCountries.length > 0 && (
              <DetailField label="Country">
                {rec.originCountries.join(", ")}
              </DetailField>
            )}
            {rec.creators.length > 0 && (
              <DetailField label={rec.mediaType === "movie" ? "Directed by" : "Created by"}>
                {rec.creators.join(", ")}
              </DetailField>
            )}
            {rec.cast.length > 0 && (
              <DetailField label="Cast">{rec.cast.join(", ")}</DetailField>
            )}
            {rec.keywords.length > 0 && (
              <DetailField label="Keywords">{rec.keywords.join(", ")}</DetailField>
            )}
            {rec.source && <DetailField label="Source">{rec.source}</DetailField>}
            {rec.confidence !== null && (
              <DetailField label="Confidence">
                {Math.round(rec.confidence * 100)}%
              </DetailField>
            )}
          </dl>
        </section>

        {scores && (
          <section className="page-section">
            <h2 className="section-title">Shortlist scores</h2>
            <div className="score-list">
              <ScoreRow label="Taste match" value={scores.tasteMatch} />
              <ScoreRow label="Novelty" value={scores.novelty} />
              <ScoreRow label="Effort fit" value={scores.effortFit} />
              <ScoreRow label="Composite" value={scores.composite} />
            </div>
            {scores.risks.length > 0 && (
              <ul className="rec-caveats detail-risks">
                {scores.risks.map((risk) => (
                  <li key={risk}>{risk}</li>
                ))}
              </ul>
            )}
          </section>
        )}

        <section className="page-section">
          <h2 className="section-title">Timeline</h2>
          <div className="timeline">
            <TimelineRow label="Recommended" at={rec.recommendedAt} />
            {rec.notifiedAt !== null && (
              <TimelineRow label="Notified" at={rec.notifiedAt} />
            )}
            {rec.startedAt !== null && (
              <TimelineRow label="Started watching" at={rec.startedAt} />
            )}
            {rec.resolvedAt !== null && (
              <TimelineRow
                label={`Resolved (${REC_STATUS_LABELS[rec.status]})`}
                at={rec.resolvedAt}
              />
            )}
            {rec.feedback && rec.feedbackAt !== null && (
              <TimelineRow
                label={`Feedback: ${
                  REC_FEEDBACK_ACTIONS.find((a) => a.value === rec.feedback)?.label ??
                  rec.feedback
                }`}
                at={rec.feedbackAt}
              />
            )}
          </div>
        </section>
      </div>
    </>
  );
}
