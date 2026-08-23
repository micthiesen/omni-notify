import { useEffect, useMemo, useState } from "react";
import {
  fetchLivestreamIntelligenceDetails,
  type LivestreamIntelligenceDetails,
  type LivestreamIntelligenceEvent,
  type LivestreamPipelineStage,
  type LivestreamStageDiagnostic,
} from "../api";
import { useNow } from "../hooks/useNow";
import { useLiveData } from "../live";
import { Link } from "../router";
import { formatDuration, formatRelative } from "../utils/format";

type TimelineFilter = "key" | "all" | "voice" | "alerts" | "errors";

const STAGES: Array<{ key: LivestreamPipelineStage; label: string }> = [
  { key: "metadata", label: "Metadata" },
  { key: "voice", label: "Voice Detection" },
  { key: "summary", label: "Now Summary" },
  { key: "alert", label: "Alerts" },
];

const FILTERS: Array<{ key: TimelineFilter; label: string }> = [
  { key: "key", label: "Key Events" },
  { key: "all", label: "All" },
  { key: "voice", label: "Voice" },
  { key: "alerts", label: "Alerts" },
  { key: "errors", label: "Errors" },
];

function moneyFromCents(cents: number): string {
  return `$${(cents / 100).toFixed(cents < 1 ? 4 : 2)}`;
}

function stageStatus(stage?: LivestreamStageDiagnostic): string {
  if (!stage) return "Not Run";
  if (stage.eligible === false) return "Not Eligible";
  return stage.status.replace(/_/g, " ");
}

function metricLabel(key: string): string {
  return key
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/^./, (value) => value.toUpperCase());
}

function PipelineStageCard({
  label,
  stage,
  now,
}: {
  label: string;
  stage?: LivestreamStageDiagnostic;
  now: number;
}) {
  const status = stageStatus(stage);
  return (
    <article className={`intelligence-stage-card stage-${stage?.status ?? "idle"}`}>
      <div className="intelligence-stage-heading">
        <h3>{label}</h3>
        <span className="intelligence-stage-status">{status}</span>
      </div>
      <p>{stage?.detail ?? "No diagnostic state has been recorded yet."}</p>
      <div className="meta-row intelligence-stage-meta">
        {stage?.finishedAt && <span>Last {formatRelative(stage.finishedAt)}</span>}
        {stage?.status === "running" && stage.startedAt && (
          <span>Running {formatDuration(now - stage.startedAt)}</span>
        )}
        {stage?.nextAt && (
          <span>
            {stage.nextAt <= now ? "Due" : "Next"} {formatRelative(stage.nextAt)}
          </span>
        )}
        {stage?.durationMs !== undefined && (
          <span>{formatDuration(stage.durationMs)} processing</span>
        )}
      </div>
      {stage?.metrics && Object.keys(stage.metrics).length > 0 && (
        <dl className="intelligence-metrics">
          {Object.entries(stage.metrics).map(([key, value]) => (
            <div key={key}>
              <dt>{metricLabel(key)}</dt>
              <dd>{typeof value === "number" ? Math.round(value * 1000) / 1000 : String(value)}</dd>
            </div>
          ))}
        </dl>
      )}
    </article>
  );
}

function eventVisible(event: LivestreamIntelligenceEvent, filter: TimelineFilter): boolean {
  if (filter === "all") return true;
  if (filter === "voice") return event.kind === "voice";
  if (filter === "alerts") return event.kind === "alert" || event.kind === "feedback";
  if (filter === "errors") return event.status === "error";
  return event.kind !== "session" || event.status !== "info";
}

function TimelineEvent({ event }: { event: LivestreamIntelligenceEvent }) {
  return (
    <li className={`intelligence-event event-${event.status}`}>
      <span className="intelligence-event-dot" aria-hidden="true" />
      <div className="intelligence-event-body">
        <div className="intelligence-event-heading">
          <strong>{event.title}</strong>
          <span>{formatRelative(event.createdAt)}</span>
        </div>
        {event.detail && <p>{event.detail}</p>}
        <div className="meta-row">
          <span>{event.kind.replace(/_/g, " ")}</span>
          <span>{event.status}</span>
          {event.durationMs !== undefined && (
            <span>{formatDuration(event.durationMs)} processing</span>
          )}
          {event.costCents !== undefined && (
            <span>{event.costCents === 0 ? "$0 local" : moneyFromCents(event.costCents)}</span>
          )}
        </div>
        {event.metrics && Object.keys(event.metrics).length > 0 && (
          <details className="intelligence-event-evidence">
            <summary>Evidence</summary>
            <dl className="intelligence-metrics">
              {Object.entries(event.metrics).map(([key, value]) => (
                <div key={key}>
                  <dt>{metricLabel(key)}</dt>
                  <dd>{typeof value === "number" ? Math.round(value * 1000) / 1000 : String(value)}</dd>
                </div>
              ))}
            </dl>
          </details>
        )}
      </div>
    </li>
  );
}

export default function LivestreamIntelligencePage({
  streamerId,
}: {
  streamerId: string;
}) {
  const { snapshot } = useLiveData();
  const now = useNow(1_000);
  const [details, setDetails] = useState<LivestreamIntelligenceDetails | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<TimelineFilter>("key");
  const streamer = snapshot?.streamers.find((item) => item.id === streamerId);

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      fetchLivestreamIntelligenceDetails(streamerId)
        .then((next) => {
          if (!cancelled) {
            setDetails(next);
            setError(null);
          }
        })
        .catch((reason) => {
          if (!cancelled) {
            setError(reason instanceof Error ? reason.message : "Failed to load diagnostics");
          }
        });
    };
    refresh();
    const timer = window.setInterval(refresh, 10_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [streamerId]);

  useEffect(() => {
    if (streamer) document.title = `${streamer.displayName} Intelligence · Omni Notify`;
  }, [streamer]);

  const events = useMemo(
    () => details?.events.filter((event) => eventVisible(event, filter)) ?? [],
    [details, filter],
  );

  if (!snapshot || (!details && !error)) return <div className="loading">Loading…</div>;
  if (!streamer) return <div className="error-banner">Streamer not found.</div>;
  if (!details) return <div className="error-banner">{error}</div>;

  const { intelligence, diagnostics, runtime } = details;
  const queueTotal = runtime
    ? Object.values(runtime.queues).reduce(
        (total, queue) => total + queue.running + queue.queued,
        0,
      )
    : 0;
  const hasStageError = Object.values(diagnostics?.stages ?? {}).some(
    (stage) => stage?.status === "error",
  );
  const budgetPercent = runtime
    ? runtime.budget.limitCents > 0
      ? Math.min(100, (runtime.budget.spentCents / runtime.budget.limitCents) * 100)
      : 100
    : 0;

  return (
    <>
      <div className="page-header intelligence-details-header">
        <div className="page-header-stack">
          <Link className="back-link" to={`/streamers/${encodeURIComponent(streamerId)}`}>
            ← {streamer.displayName}
          </Link>
          <h1>Intelligence Details</h1>
          <p className="page-subtitle">
            What the pipeline is doing, why it made each decision, and what it cost.
          </p>
        </div>
        <span className={`intelligence-live-state ${streamer.live ? "is-live" : ""}`}>
          {streamer.live ? "Live Session" : "Last Session"}
        </span>
      </div>

      {error && <div className="error-banner">Latest refresh failed: {error}</div>}

      <section className="intelligence-health-grid" aria-label="Intelligence health">
        <article>
          <span className="stat-label">Pipeline</span>
          <strong>{runtime ? (hasStageError ? "Needs Attention" : "Healthy") : "Unavailable"}</strong>
          <span>
            {hasStageError
              ? "A pipeline stage failed"
              : queueTotal === 0
                ? "Queues clear"
                : `${queueTotal} queued or running`}
          </span>
        </article>
        <article>
          <span className="stat-label">Voice Model</span>
          <strong>{runtime?.voiceprintLoaded ? "Ready" : "Unavailable"}</strong>
          <span>
            {runtime
              ? `${runtime.activeVoiceTargetCount} of ${runtime.activeStreamCount} live targets`
              : "No runtime connection"}
          </span>
        </article>
        <article>
          <span className="stat-label">Monthly Budget</span>
          <strong>
            {runtime
              ? `${moneyFromCents(runtime.budget.spentCents)} of ${moneyFromCents(runtime.budget.limitCents)}`
              : "—"}
          </strong>
          <div className="intelligence-budget-track" aria-label={`${Math.round(budgetPercent)}% used`}>
            <span style={{ width: `${budgetPercent}%` }} />
          </div>
        </article>
        <article>
          <span className="stat-label">Timeline</span>
          <strong>{details.events.length} recent events</strong>
          <span>{intelligence?.chapters.length ?? 0} topic chapters retained</span>
        </article>
      </section>

      <section className="page-section">
        <h2 className="section-title">Current Pipeline</h2>
        <div className="intelligence-stage-grid">
          {STAGES.map((stage) => (
            <PipelineStageCard
              key={stage.key}
              label={stage.label}
              stage={diagnostics?.stages[stage.key]}
              now={now}
            />
          ))}
        </div>
      </section>

      {intelligence && (
        <section className="page-section intelligence-current-evidence">
          <h2 className="section-title">Current Evidence</h2>
          <div className="intelligence-evidence-grid">
            <article>
              <h3>Semantic Read</h3>
              <strong>{intelligence.semantic?.headline ?? "Not classified yet"}</strong>
              {intelligence.semantic?.reason && <p>{intelligence.semantic.reason}</p>}
              {intelligence.semantic?.topics && (
                <div className="intelligence-topic-list">
                  {intelligence.semantic.topics.map((topic) => <span key={topic}>{topic}</span>)}
                </div>
              )}
            </article>
            <article>
              <h3>Latest Transcript Window</h3>
              {intelligence.summary ? (
                <>
                  <strong>{intelligence.summary.topic}</strong>
                  <p className="intelligence-transcript-excerpt">
                    {intelligence.summary.transcriptExcerpt}
                  </p>
                  <div className="meta-row">
                    <span>{Math.round(intelligence.summary.confidence * 100)}% confidence</span>
                    <span>{intelligence.summary.windowSeconds}s window</span>
                  </div>
                </>
              ) : (
                <p>No transcript-backed summary has been produced for this session.</p>
              )}
            </article>
          </div>
        </section>
      )}

      <section className="page-section">
        <div className="intelligence-timeline-header">
          <h2 className="section-title">Decision Timeline</h2>
          <div className="intelligence-filter-row" aria-label="Timeline filters">
            {FILTERS.map((item) => (
              <button
                key={item.key}
                type="button"
                className={`chip-btn ${filter === item.key ? "active" : ""}`}
                onClick={() => setFilter(item.key)}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
        {events.length > 0 ? (
          <ol className="intelligence-event-list">
            {events.map((event) => <TimelineEvent key={event.eventId} event={event} />)}
          </ol>
        ) : (
          <div className="no-data">No events match this filter yet.</div>
        )}
      </section>
    </>
  );
}
