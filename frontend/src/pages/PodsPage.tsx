import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  dismissPressPodsJob,
  fetchPressPods,
  fetchRunLogs,
  type PressPodsEpisode,
  type PressPodsJob,
  retryPressPodsJob,
  submitPressPodsUrl,
  type TaskRun,
} from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { LogViewer } from "../components/LogViewer";
import { ShowMoreButton, useShowMore } from "../components/ShowMore";
import { useLiveData } from "../live";
import { formatAbsolute } from "../utils/format";

const JOB_STATUS_LABELS: Record<PressPodsJob["status"], string> = {
  queued: "Queued",
  processing: "Processing",
  failed: "Failed",
};

function formatAudioDuration(seconds: number | null): string | null {
  if (seconds === null || !Number.isFinite(seconds)) return null;
  const total = Math.round(seconds);
  const mins = Math.floor(total / 60);
  const secs = total % 60;
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

function formatCost(costCents: number | null): string | null {
  if (costCents === null) return null;
  return `US$${(costCents / 100).toFixed(2)}`;
}

function retrieverSummary(episode: PressPodsEpisode): string | null {
  if (!episode.retrieverAttempts) return episode.retrieverName;
  const ok = episode.retrieverAttempts.filter((a) => a.success).length;
  return `${episode.retrieverName} (${ok}/${episode.retrieverAttempts.length} retrievers)`;
}

export default function PodsPage() {
  const { snapshot } = useLiveData();
  const [episodes, setEpisodes] = useState<PressPodsEpisode[] | null>(null);
  const [jobs, setJobs] = useState<PressPodsJob[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [logRun, setLogRun] = useState<TaskRun | null>(null);

  const load = useCallback(() => {
    fetchPressPods()
      .then((res) => {
        setEpisodes(res.episodes);
        setJobs(res.jobs);
        setError(null);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : "Failed to load episodes");
      });
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Refresh whenever a PressPods run settles (episodes/jobs will have changed).
  const pressPodsRun = snapshot?.tasks.find((t) => t.name === "PressPods")?.lastRun;
  const runKey = pressPodsRun ? `${pressPodsRun.runId}:${pressPodsRun.status}` : null;
  useEffect(() => {
    if (runKey !== null) load();
  }, [runKey, load]);

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed || submitting) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      await submitPressPodsUrl(trimmed);
      setUrl("");
      load();
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : "Failed to submit URL");
    } finally {
      setSubmitting(false);
    }
  };

  const onRetry = async (jobId: string) => {
    try {
      await retryPressPodsJob(jobId);
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to retry job");
    }
  };

  const onDismiss = async (jobId: string) => {
    try {
      await dismissPressPodsJob(jobId);
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to dismiss job");
    }
  };

  const openLogs = async (runId: string) => {
    try {
      const { run } = await fetchRunLogs(runId);
      setLogRun(run);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load logs");
    }
  };

  const { visible, hasMore, remaining, showMore } = useShowMore(
    episodes ?? [],
    20,
    null,
  );

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>PressPods</h1>
          <p className="page-subtitle">
            Articles converted to podcast episodes, read aloud by a robot.
          </p>
        </div>
      </div>

      <form className="pods-submit" onSubmit={onSubmit}>
        <input
          type="url"
          className="pods-submit-input"
          placeholder="https://example.com/article"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          disabled={submitting}
        />
        <button
          type="submit"
          className="pods-submit-btn"
          disabled={submitting || url.trim() === ""}
        >
          {submitting ? "Submitting…" : "Create episode"}
        </button>
      </form>
      {submitError && <div className="pods-submit-error">{submitError}</div>}

      {jobs.length > 0 && (
        <section className="pods-jobs">
          <h2 className="section-title">In progress</h2>
          {jobs.map((job) => (
            <div key={job.jobId} className={`pods-job pods-job-${job.status}`}>
              <div className="pods-job-main">
                <span className={`status-dot status-${jobStatusDot(job.status)}`} />
                <span className="pods-job-url" title={job.url}>
                  {job.url}
                </span>
              </div>
              <div className="meta-row pods-job-meta">
                <span>{JOB_STATUS_LABELS[job.status]}</span>
                <span>{formatAbsolute(job.createdAt)}</span>
                {job.attempts > 0 && <span>Attempt {job.attempts}</span>}
              </div>
              {job.lastError && <div className="run-error">{job.lastError}</div>}
              <div className="pods-job-actions">
                {job.status === "failed" && (
                  <>
                    <button
                      type="button"
                      className="chip-btn"
                      onClick={() => onRetry(job.jobId)}
                    >
                      Retry
                    </button>
                    <button
                      type="button"
                      className="chip-btn"
                      onClick={() => onDismiss(job.jobId)}
                    >
                      Dismiss
                    </button>
                  </>
                )}
                {job.lastRunId && (
                  <button
                    type="button"
                    className="chip-btn"
                    onClick={() => job.lastRunId && openLogs(job.lastRunId)}
                  >
                    Logs
                  </button>
                )}
              </div>
            </div>
          ))}
        </section>
      )}

      {episodes === null && error === null && <div className="loading">Loading…</div>}
      {error && <div className="error">{error}</div>}
      {episodes !== null && episodes.length === 0 && jobs.length === 0 && (
        <div className="muted">
          No episodes yet. Submit an article URL above to create one.
        </div>
      )}

      {episodes !== null && episodes.length > 0 && (
        <div className="pods-feed">
          {visible.map((episode) => (
            <article key={episode.episodeId} className="pods-card">
              <div className="pods-card-body">
                <ImageWithFallback
                  src={episode.leadImageUrl}
                  alt=""
                  className="pods-card-art"
                  placeholderClassName="pods-card-art-placeholder"
                  placeholder={
                    <svg
                      width="24"
                      height="24"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      aria-hidden="true"
                    >
                      <rect x="9" y="2" width="6" height="12" rx="3" />
                      <path d="M5 10a7 7 0 0 0 14 0" />
                      <line x1="12" y1="17" x2="12" y2="21" />
                      <line x1="8" y1="21" x2="16" y2="21" />
                    </svg>
                  }
                  loading="lazy"
                />
                <div className="pods-card-info">
                  <h2 className="pods-card-title">{episode.title}</h2>
                  <div className="meta-row pods-card-meta">
                    <span>{formatAbsolute(episode.createdAt)}</span>
                    {formatAudioDuration(episode.durationSeconds) && (
                      <span>{formatAudioDuration(episode.durationSeconds)}</span>
                    )}
                    {episode.voiceName && <span>{episode.voiceName}</span>}
                    {formatCost(episode.costCents) && (
                      <span>{formatCost(episode.costCents)}</span>
                    )}
                    {retrieverSummary(episode) && (
                      <span title="Winning retriever">
                        {retrieverSummary(episode)}
                      </span>
                    )}
                  </div>
                  <div className="pods-card-links">
                    <a
                      href={episode.articleUrl}
                      target="_blank"
                      rel="noreferrer"
                      className="pods-card-source"
                    >
                      {episode.domain ?? "Article"} ↗
                    </a>
                    {episode.runId && (
                      <button
                        type="button"
                        className="pods-card-logs"
                        onClick={() => episode.runId && openLogs(episode.runId)}
                      >
                        Logs
                      </button>
                    )}
                  </div>
                  {episode.excerpt && (
                    <p className="pods-card-excerpt">{episode.excerpt}</p>
                  )}
                </div>
              </div>
              {/* biome-ignore lint/a11y/useMediaCaption: TTS audio has no captions */}
              <audio
                className="pods-card-audio"
                controls
                preload="none"
                src={episode.audioUrl}
              />
            </article>
          ))}
          {hasMore && <ShowMoreButton remaining={remaining} onClick={showMore} />}
        </div>
      )}

      {logRun && <LogViewer run={logRun} onClose={() => setLogRun(null)} />}
    </>
  );
}

function jobStatusDot(status: PressPodsJob["status"]): string {
  if (status === "failed") return "error";
  if (status === "processing") return "running";
  return "none";
}
