import { useEffect, useState } from "react";
import {
  fetchPodcastRecommendation,
  sendPodcastRecommendationFeedback,
} from "../api";
import type { PodcastFeedback, PodcastRecommendation } from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { Toast, useToast } from "../components/Toast";
import { Link } from "../router";
import { formatAbsoluteWithYear, formatDateOnly } from "../utils/format";
import { PODCAST_FEEDBACK_ACTIONS, PODCAST_STATUS_LABELS } from "../utils/recLabels";

function formatEpisodeDuration(minutes: number): string {
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

export default function PodcastDetailPage({ id }: { id: string }) {
  const [rec, setRec] = useState<PodcastRecommendation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const { toast, showToast } = useToast();

  useEffect(() => {
    let cancelled = false;
    fetchPodcastRecommendation(id)
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

  const handleFeedback = async (feedback: PodcastFeedback) => {
    setSaving(true);
    try {
      const result = await sendPodcastRecommendationFeedback(id, feedback);
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
      <Link to="/podcasts" className="detail-back">
        ← All podcast picks
      </Link>
      <Toast toast={toast} />

      <div className="detail-head">
        <ImageWithFallback
          src={rec.artworkUrl ?? null}
          alt={`${rec.showTitle} artwork`}
          className="detail-art detail-art-square"
          placeholderClassName="detail-art-placeholder"
          placeholder="🎧"
        />
        <div className="detail-head-body">
          <h1 className="detail-title">{rec.episodeTitle}</h1>
          <div className="podrec-show">{rec.showTitle}</div>
          <div className="detail-badges">
            <span className={`status-chip status-chip-${rec.status}`}>
              {PODCAST_STATUS_LABELS[rec.status]}
            </span>
            {(rec.queueResult === "queued" ||
              rec.queueResult === "already_queued") && (
              <span
                className="podrec-queued"
                title="This episode is waiting in your Castro queue"
              >
                🎧 In Castro queue
              </span>
            )}
          </div>
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
              {PODCAST_FEEDBACK_ACTIONS.map((action) => (
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
            <DetailField label="Released">
              {formatDateOnly(rec.publishedAt)}
            </DetailField>
            {rec.durationMinutes != null && (
              <DetailField label="Duration">
                {formatEpisodeDuration(rec.durationMinutes)}
              </DetailField>
            )}
            {rec.discoveredVia && (
              <DetailField label="Discovered via">{rec.discoveredVia}</DetailField>
            )}
            {rec.confidence != null && (
              <DetailField label="Confidence">
                {Math.round(rec.confidence * 100)}%
              </DetailField>
            )}
            {rec.itunesId != null && (
              <DetailField label="iTunes ID">{rec.itunesId}</DetailField>
            )}
            <DetailField label="Feed">
              <a
                href={rec.feedUrl}
                target="_blank"
                rel="noreferrer"
                className="detail-feed-url"
              >
                {rec.feedUrl}
              </a>
            </DetailField>
          </dl>
        </section>

        {scores && (
          <section className="page-section">
            <h2 className="section-title">Shortlist scores</h2>
            <div className="score-list">
              <ScoreRow label="Taste match" value={scores.tasteMatch} />
              <ScoreRow label="Novelty" value={scores.novelty} />
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
            {rec.notifiedAt != null && (
              <TimelineRow label="Notified" at={rec.notifiedAt} />
            )}
            {rec.feedback && rec.feedbackAt != null && (
              <TimelineRow
                label={`Feedback: ${
                  PODCAST_FEEDBACK_ACTIONS.find((a) => a.value === rec.feedback)
                    ?.label ?? rec.feedback
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
