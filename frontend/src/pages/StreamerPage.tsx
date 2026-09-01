import { useEffect, useMemo, useState } from "react";
import { Bar, BarChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import {
  fetchStreamerMetrics,
  fetchStreamerSessions,
  submitLivestreamFeedback,
} from "../api";
import type { StreamerMetrics, StreamerView, StreamSession } from "../api";
import { DggPresenceTag } from "../components/LiveNow";
import { PlatformIcon } from "../components/PlatformIcon";
import { ShowMoreButton, useShowMore } from "../components/ShowMore";
import { useNow } from "../hooks/useNow";
import { useLiveData } from "../live";
import { Link } from "../router";
import {
  formatCompactNumber,
  formatDateOnly,
  formatDuration,
  formatRelative,
  formatUptime,
} from "../utils/format";

type Range = "30d" | "90d" | "all";

const RANGE_DAYS: Record<Range, number | null> = { "30d": 30, "90d": 90, all: null };

function dateString(d: Date): string {
  return d.toISOString().slice(0, 10);
}

function daysAgo(days: number): string {
  const d = new Date();
  d.setDate(d.getDate() - days);
  return dateString(d);
}

interface DayPoint {
  date: string;
  maxViewers: number;
  streamed: boolean;
}

/**
 * Expand sparse daily buckets into a continuous day series so off days show
 * as gaps between bars instead of being silently compressed away.
 */
function buildDaySeries(metrics: StreamerMetrics, range: Range): DayPoint[] {
  const byDate = new Map(metrics.dailyBuckets.map((b) => [b.date, b.maxViewers]));
  const days = RANGE_DAYS[range];
  const firstBucket = metrics.dailyBuckets[0]?.date;
  if (!firstBucket) return [];
  const rangeStart = days === null ? firstBucket : daysAgo(days - 1);
  // Never start before tracking began: leading days would read as "no stream"
  // when we simply weren't watching yet.
  const start = rangeStart < firstBucket ? firstBucket : rangeStart;
  const today = dateString(new Date());

  const series: DayPoint[] = [];
  const cursor = new Date(`${start}T00:00:00Z`);
  while (dateString(cursor) <= today) {
    const date = dateString(cursor);
    const maxViewers = byDate.get(date);
    series.push({
      date,
      maxViewers: maxViewers ?? 0,
      streamed: maxViewers !== undefined,
    });
    cursor.setUTCDate(cursor.getUTCDate() + 1);
  }
  return series;
}

function formatDayTick(date: string): string {
  return new Date(`${date}T00:00:00`).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
}

function formatDayFull(date: string): string {
  return new Date(`${date}T00:00:00`).toLocaleDateString("en-US", {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function windowMax(metrics: StreamerMetrics, days: number): number {
  const cutoff = daysAgo(days);
  let max = 0;
  for (const bucket of metrics.dailyBuckets) {
    if (bucket.date >= cutoff && bucket.maxViewers > max) max = bucket.maxViewers;
  }
  return max;
}

function StreamerStats({ metrics }: { metrics: StreamerMetrics }) {
  const daysStreamed30 = metrics.dailyBuckets.filter(
    (b) => b.date >= daysAgo(30),
  ).length;
  const highs: { label: string; value: number; detail?: string }[] = [
    { label: "7-Day High", value: windowMax(metrics, 7) },
    { label: "30-Day High", value: windowMax(metrics, 30) },
    { label: "90-Day High", value: windowMax(metrics, 90) },
    {
      label: "All-Time Record",
      value: metrics.allTimeMax,
      detail: metrics.allTimeMaxTimestamp
        ? formatDateOnly(metrics.allTimeMaxTimestamp)
        : undefined,
    },
  ];

  return (
    <div className="stat-strip">
      {highs.map((h) => (
        <div key={h.label} className="stat-tile">
          <span className="stat-label">{h.label}</span>
          <span className="stat-value">
            {h.value > 0 ? formatCompactNumber(h.value) : "—"}
          </span>
          {h.detail && <span className="stat-detail">{h.detail}</span>}
        </div>
      ))}
      <div className="stat-tile">
        <span className="stat-label">Days Streamed</span>
        <span className="stat-value">{daysStreamed30}</span>
        <span className="stat-detail">of last 30</span>
      </div>
    </div>
  );
}

function PlatformStats({ metrics }: { metrics: StreamerMetrics }) {
  if (metrics.platforms.length === 0) return null;
  return (
    <section className="page-section">
      <h2 className="section-title">Platform Records</h2>
      <div className="stat-strip">
        {metrics.platforms.map((source) => (
          <div key={`${source.platform}:${source.username}`} className="stat-tile">
            <span className="stat-label">
              <PlatformIcon platform={source.platform} size={13} /> {source.platform}
            </span>
            <span className="stat-value">
              {source.allTimeMax > 0 ? formatCompactNumber(source.allTimeMax) : "—"}
            </span>
            <span className="stat-detail">{source.username} all-time</span>
          </div>
        ))}
      </div>
    </section>
  );
}

function ViewerChart({ metrics }: { metrics: StreamerMetrics }) {
  const [range, setRange] = useState<Range>("30d");
  const series = useMemo(() => buildDaySeries(metrics, range), [metrics, range]);
  const ranges: Range[] = ["30d", "90d", "all"];

  return (
    <section className="page-section">
      <h2 className="section-title">
        Peak Viewers by Day
        <span className="range-buttons">
          {ranges.map((r) => (
            <button
              key={r}
              type="button"
              className={`range-btn ${range === r ? "active" : ""}`}
              onClick={() => setRange(r)}
            >
              {r === "all" ? "All" : r}
            </button>
          ))}
        </span>
      </h2>
      {series.every((d) => !d.streamed) ? (
        <div className="no-data">No streams in this range</div>
      ) : (
        <div className="chart-container">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart
              accessibilityLayer={false}
              data={series}
              margin={{ top: 8, right: 16, left: 4, bottom: 8 }}
            >
              <XAxis
                dataKey="date"
                tick={{ fill: "#8888a8", fontSize: 12 }}
                tickLine={{ stroke: "#3a3a5a" }}
                axisLine={{ stroke: "#3a3a5a" }}
                tickFormatter={formatDayTick}
                minTickGap={40}
              />
              <YAxis
                tick={{ fill: "#8888a8", fontSize: 12 }}
                tickLine={{ stroke: "#3a3a5a" }}
                axisLine={{ stroke: "#3a3a5a" }}
                tickFormatter={(v: number) => formatCompactNumber(v)}
                width={44}
              />
              <Tooltip
                cursor={{ fill: "rgba(56, 189, 248, 0.08)" }}
                content={({ active, payload }) => {
                  if (!active || !payload?.length) return null;
                  const point = payload[0].payload as DayPoint;
                  return (
                    <div className="custom-tooltip">
                      <div className="tooltip-label">{formatDayFull(point.date)}</div>
                      <div className="tooltip-row">
                        {point.streamed
                          ? `${point.maxViewers.toLocaleString()} peak viewers`
                          : "No stream"}
                      </div>
                    </div>
                  );
                }}
              />
              <Bar
                dataKey="maxViewers"
                fill="#38bdf8"
                radius={[3, 3, 0, 0]}
                maxBarSize={28}
              />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}
    </section>
  );
}

const MAX_SESSION_ROWS = 25;

function formatSessionDate(timestamp: number): string {
  return new Date(timestamp).toLocaleDateString("en-US", {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
}

function formatSessionTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString("en-US", {
    hour: "numeric",
    minute: "2-digit",
  });
}

function RecentSessions({ sessions }: { sessions: StreamSession[] }) {
  const {
    visible: shown,
    hasMore,
    remaining,
    showMore,
  } = useShowMore(sessions, MAX_SESSION_ROWS);
  return (
    <section className="page-section">
      <h2 className="section-title">
        Recent Streams
        <span className="section-count">{sessions.length}</span>
      </h2>
      <ul className="session-list">
        {shown.map((session) => (
          <li key={`${session.startedAt}-${session.endedAt}`} className="session-row">
            <div className="session-when">
              <span className="session-date">
                {formatSessionDate(session.startedAt)}
              </span>
              <span className="session-time">
                {formatSessionTime(session.startedAt)} –{" "}
                {formatSessionTime(session.endedAt)}
              </span>
            </div>
            <div className="session-title" title={session.title}>
              <PlatformIcon platform={session.platform} size={13} />
              <span>{session.title || "Untitled stream"}</span>
            </div>
            <div className="session-stats">
              <span>{formatDuration(session.durationMs)}</span>
              <span className="session-peak">
                {session.peakViewers > 0
                  ? `${formatCompactNumber(session.peakViewers)} peak`
                  : "—"}
              </span>
            </div>
          </li>
        ))}
      </ul>
      {hasMore && <ShowMoreButton remaining={remaining} onClick={showMore} />}
    </section>
  );
}

function StreamerHeader({ streamer }: { streamer: StreamerView }) {
  const now = useNow(1000);

  return (
    <div className="page-header streamer-header">
      <div>
        <h1>
          {streamer.displayName}
          {streamer.live && (
            <span className="live-badge">
              <span className="live-badge-dot" />
              LIVE
            </span>
          )}
          <DggPresenceTag dgg={streamer.dgg} />
        </h1>
        {streamer.live ? (
          <div className="streamer-sub">
            <span className="streamer-title-text">{streamer.title}</span>
            <span className="muted meta-row">
              <span>{formatUptime(now - streamer.startedAt)}</span>
              {streamer.viewerCount !== null ? (
                <span>{formatCompactNumber(streamer.viewerCount)} watching</span>
              ) : (
                streamer.maxViewerCount > 0 && (
                  <span>
                    {formatCompactNumber(streamer.maxViewerCount)} peak viewers this
                    stream
                  </span>
                )
              )}
              {streamer.sources.length > 1 &&
                streamer.sources.map(
                  (source) =>
                    source.viewerCount !== null && (
                      <span key={`${source.platform}:${source.username}`}>
                        <PlatformIcon platform={source.platform} size={12} />{" "}
                        {formatCompactNumber(source.viewerCount)} {source.platform}
                      </span>
                    ),
                )}
              {streamer.category && <span>{streamer.category}</span>}
            </span>
          </div>
        ) : (
          <div className="streamer-sub muted">
            {streamer.lastEndedAt !== null
              ? `Last live ${formatRelative(streamer.lastEndedAt)}${
                  streamer.lastStartedAt !== null
                    ? ` for ${formatDuration(streamer.lastEndedAt - streamer.lastStartedAt)}`
                    : ""
                }${
                  streamer.lastMaxViewerCount
                    ? `, peak ${formatCompactNumber(streamer.lastMaxViewerCount)} viewers`
                    : ""
                }`
              : "No streams seen yet"}
          </div>
        )}
      </div>
      <div className="streamer-bindings">
        <Link
          className="binding-chip intelligence-details-link"
          to={`/streamers/${encodeURIComponent(streamer.id)}/intelligence`}
        >
          Intelligence Details
        </Link>
        {streamer.bindings.map((binding) => (
          <a
            key={`${binding.platform}:${binding.username}`}
            className="binding-chip"
            href={binding.url}
            target="_blank"
            rel="noreferrer"
          >
            <PlatformIcon platform={binding.platform} size={14} />
            <span>{binding.username}</span>
          </a>
        ))}
      </div>
    </div>
  );
}

function LivestreamIntelligencePanel({ streamer }: { streamer: StreamerView }) {
  const [submitted, setSubmitted] = useState<string | null>(null);
  const alertId = streamer.live
    ? streamer.intelligence?.latestAlert?.alertId
    : undefined;
  useEffect(() => setSubmitted(null), [alertId]);
  if (!streamer.live || !streamer.intelligence) return null;
  const intelligence = streamer.intelligence;
  const chapters = intelligence.chapters.slice(-8).reverse();
  const submit = (verdict: "useful" | "not_useful" | "false_positive") => {
    const alert = intelligence.latestAlert;
    if (!alert) return;
    setSubmitted(verdict);
    void submitLivestreamFeedback(streamer.id, alert.alertId, verdict).catch(() =>
      setSubmitted(null),
    );
  };

  return (
    <section className="page-section intelligence-panel">
      <h2 className="section-title intelligence-panel-title">
        Live Intelligence
        <Link
          className="section-action-link"
          to={`/streamers/${encodeURIComponent(streamer.id)}/intelligence`}
        >
          Details ›
        </Link>
      </h2>
      <div className="intelligence-summary-card">
        <div className="intelligence-summary-heading">
          <strong>{intelligence.summary?.topic ?? "Current Read"}</strong>
          <span className="intelligence-score">
            Relevance {intelligence.relevanceScore}
          </span>
        </div>
        <p>
          {intelligence.summary?.text ??
            intelligence.semantic?.headline ??
            "Building a current summary…"}
        </p>
        <div className="meta-row">
          {intelligence.summary && (
            <span>Updated {formatRelative(intelligence.summary.updatedAt)}</span>
          )}
          {intelligence.trend?.anomalous && <span>{intelligence.trend.reason}</span>}
          {intelligence.destinyPresence?.state === "confirmed" && (
            <span>
              Destiny detected,{" "}
              {Math.round(intelligence.destinyPresence.confidence * 100)}% confidence
            </span>
          )}
        </div>
        {intelligence.relevanceReasons.length > 0 && (
          <div className="intelligence-reasons">
            {intelligence.relevanceReasons.map((reason) => (
              <span key={reason}>{reason}</span>
            ))}
          </div>
        )}
      </div>
      {chapters.length > 0 && (
        <div className="intelligence-chapters">
          <h3>Recent Topics</h3>
          {chapters.map((chapter) => (
            <div key={chapter.chapterId} className="intelligence-chapter">
              <div>
                <strong>{chapter.title}</strong>
                <span>{formatRelative(chapter.startedAt)}</span>
              </div>
              <p>{chapter.summary}</p>
            </div>
          ))}
        </div>
      )}
      {intelligence.latestAlert && (
        <div className="intelligence-alert-feedback">
          <div>
            <strong>Latest Alert: {intelligence.latestAlert.title}</strong>
            <span>{intelligence.latestAlert.reason}</span>
          </div>
          {submitted ? (
            <span className="muted">Feedback saved</span>
          ) : (
            <div className="intelligence-feedback-actions">
              <button type="button" onClick={() => submit("useful")}>
                Useful
              </button>
              <button type="button" onClick={() => submit("not_useful")}>
                Not Useful
              </button>
              <button type="button" onClick={() => submit("false_positive")}>
                False Positive
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

export default function StreamerPage({ streamerId }: { streamerId: string }) {
  const { snapshot } = useLiveData();
  const [metrics, setMetrics] = useState<StreamerMetrics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<StreamSession[] | null>(null);

  const streamer = snapshot?.streamers.find((s) => s.id === streamerId) ?? null;

  useEffect(() => {
    if (streamer) document.title = `${streamer.displayName} · Omni Notify`;
  }, [streamer]);

  useEffect(() => {
    let cancelled = false;
    fetchStreamerMetrics(streamerId)
      .then((data) => {
        if (!cancelled) setMetrics(data);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load metrics");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [streamerId]);

  useEffect(() => {
    let cancelled = false;
    fetchStreamerSessions(streamerId)
      .then((data) => {
        if (!cancelled) setSessions(data.sessions);
      })
      .catch(() => {
        // Session history is supplementary; the page stays useful without it.
        if (!cancelled) setSessions(null);
      });
    return () => {
      cancelled = true;
    };
  }, [streamerId]);

  if (snapshot === null) {
    return <div className="loading">Loading…</div>;
  }

  if (!streamer) {
    return (
      <>
        <Link to="/" className="back-link">
          ← Home
        </Link>
        <div className="error">
          <div>Unknown streamer</div>
          <div className="error-detail">
            No channel named “{streamerId}” is being monitored.
          </div>
        </div>
      </>
    );
  }

  const hasMetrics =
    metrics !== null && (metrics.dailyBuckets.length > 0 || metrics.allTimeMax > 0);

  return (
    <>
      <Link to="/" className="back-link">
        ← Home
      </Link>
      <StreamerHeader streamer={streamer} />
      <LivestreamIntelligencePanel streamer={streamer} />
      {error !== null && (
        <div className="error-inline">Failed to load viewer metrics: {error}</div>
      )}
      {metrics === null && error === null && (
        <div className="loading-inline">Loading viewer metrics…</div>
      )}
      {metrics !== null && !hasMetrics && (
        <div className="no-data">No viewer data recorded yet</div>
      )}
      {metrics !== null && hasMetrics && (
        <>
          <StreamerStats metrics={metrics} />
          <PlatformStats metrics={metrics} />
          <ViewerChart metrics={metrics} />
        </>
      )}
      {sessions !== null && sessions.length > 0 && (
        <RecentSessions sessions={sessions} />
      )}
    </>
  );
}
