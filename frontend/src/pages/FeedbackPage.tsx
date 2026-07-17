import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import {
  fetchPodcastRecommendation,
  fetchRecommendation,
  type PodcastFeedback,
  type PodcastRecommendation,
  type Recommendation,
  type RecommendationFeedback,
  sendPodcastRecommendationFeedback,
  sendRecommendationFeedback,
} from "../api";
import { Link } from "../router";

export type FeedbackKind = "recommendations" | "podcasts";

interface FeedbackOption<V extends string> {
  value: V;
  emoji: string;
  label: string;
}

const MEDIA_OPTIONS: FeedbackOption<RecommendationFeedback>[] = [
  { value: "good_pick", emoji: "👍", label: "Good pick" },
  { value: "not_for_me", emoji: "👎", label: "Not for me" },
  { value: "already_watched", emoji: "✅", label: "Already watched" },
];

const PODCAST_OPTIONS: FeedbackOption<PodcastFeedback>[] = [
  { value: "good_pick", emoji: "👍", label: "Good pick" },
  { value: "not_for_me", emoji: "👎", label: "Not for me" },
];

function Art({ src, alt }: { src: string | null; alt: string }) {
  const [broken, setBroken] = useState(false);
  if (!src || broken) {
    return <div className="feedback-art feedback-art-placeholder">🎯</div>;
  }
  return (
    <img
      className="feedback-art"
      src={src}
      alt={alt}
      onError={() => setBroken(true)}
    />
  );
}

interface ShellProps<V extends string> {
  art: ReactNode;
  title: string;
  subtitle: string | null;
  why: string | null;
  options: FeedbackOption<V>[];
  current: V | null;
  detailsTo: string;
  onSelect: (value: V) => Promise<void>;
}

function FeedbackShell<V extends string>({
  art,
  title,
  subtitle,
  why,
  options,
  current,
  detailsTo,
  onSelect,
}: ShellProps<V>) {
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const select = async (value: V) => {
    if (saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      await onSelect(value);
      setSaved(true);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : "Failed to save feedback");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="feedback-card">
      {art}
      <h1 className="feedback-title">{title}</h1>
      {subtitle && <div className="feedback-sub">{subtitle}</div>}
      {why && <p className="feedback-why">{why}</p>}
      <div className="feedback-options">
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            className={`feedback-option ${current === option.value ? "active" : ""}`}
            aria-pressed={current === option.value}
            disabled={saving}
            onClick={() => select(option.value)}
          >
            <span aria-hidden="true">{option.emoji}</span>
            {option.label}
          </button>
        ))}
      </div>
      {saved && !saveError && (
        <div className="feedback-saved">Thanks — feedback saved.</div>
      )}
      {saveError && <div className="error-inline">{saveError}</div>}
      <Link to={detailsTo} className="feedback-details-link">
        View full recommendation →
      </Link>
    </div>
  );
}

function MediaFeedback({ id }: { id: string }) {
  const [rec, setRec] = useState<Recommendation | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchRecommendation(id)
      .then((res) => {
        if (!cancelled) setRec(res.recommendation);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  if (error) return <FeedbackError error={error} />;
  if (!rec) return <div className="loading">Loading…</div>;

  return (
    <FeedbackShell
      art={
        <Art
          src={
            rec.posterPath ? `https://image.tmdb.org/t/p/w185${rec.posterPath}` : null
          }
          alt={`${rec.title} poster`}
        />
      }
      title={rec.year ? `${rec.title} (${rec.year})` : rec.title}
      subtitle={rec.mediaType === "movie" ? "Movie" : "Series"}
      why={rec.whyForUser}
      options={MEDIA_OPTIONS}
      current={rec.feedback}
      detailsTo={`/recommendations?recommendation=${encodeURIComponent(id)}`}
      onSelect={async (feedback) => {
        const res = await sendRecommendationFeedback(id, feedback);
        setRec(res.recommendation);
      }}
    />
  );
}

function PodcastFeedbackCard({ id }: { id: string }) {
  const [rec, setRec] = useState<PodcastRecommendation | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchPodcastRecommendation(id)
      .then((res) => {
        if (!cancelled) setRec(res.recommendation);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  if (error) return <FeedbackError error={error} />;
  if (!rec) return <div className="loading">Loading…</div>;

  return (
    <FeedbackShell
      art={<Art src={rec.artworkUrl ?? null} alt={`${rec.showTitle} artwork`} />}
      title={rec.episodeTitle}
      subtitle={rec.showTitle}
      why={rec.whyForUser ?? null}
      options={PODCAST_OPTIONS}
      current={rec.feedback ?? null}
      detailsTo={`/podcasts?recommendation=${encodeURIComponent(id)}`}
      onSelect={async (feedback) => {
        const res = await sendPodcastRecommendationFeedback(id, feedback);
        setRec(res.recommendation);
      }}
    />
  );
}

function FeedbackError({ error }: { error: string }) {
  return (
    <div className="error">
      <div>Couldn't load this recommendation</div>
      <div className="error-detail">{error}</div>
    </div>
  );
}

export default function FeedbackPage({ kind, id }: { kind: FeedbackKind; id: string }) {
  return (
    <div className="feedback-page">
      {kind === "recommendations" ? (
        <MediaFeedback id={id} />
      ) : (
        <PodcastFeedbackCard id={id} />
      )}
    </div>
  );
}
