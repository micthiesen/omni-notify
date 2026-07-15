import type { LiveStreamer, OfflineStreamer, StreamerView } from "../api";
import { useNow } from "../hooks/useNow";
import {
  formatCompactNumber,
  formatDuration,
  formatRelative,
  formatUptime,
} from "../utils/format";
import { PlatformIcon } from "./PlatformIcon";

function LiveStreamerCard({ streamer }: { streamer: LiveStreamer }) {
  const now = useNow(1000);
  return (
    <a
      className="live-card"
      href={streamer.primary.url}
      target="_blank"
      rel="noreferrer"
    >
      <div className="live-card-header">
        <PlatformIcon platform={streamer.primary.platform} size={16} />
        <span className="live-name">{streamer.displayName}</span>
        <span className="live-badge">
          <span className="live-badge-dot" />
          LIVE
        </span>
      </div>
      <div className="live-title">{streamer.title}</div>
      <div className="live-meta">
        <span className="live-uptime">{formatUptime(now - streamer.startedAt)}</span>
        {streamer.maxViewerCount > 0 && (
          <span>{formatCompactNumber(streamer.maxViewerCount)} peak viewers</span>
        )}
      </div>
    </a>
  );
}

function OfflinePill({ streamer }: { streamer: OfflineStreamer }) {
  const url = streamer.bindings[0]?.url;
  const lastLive =
    streamer.lastEndedAt !== null ? formatRelative(streamer.lastEndedAt) : null;
  const title =
    streamer.lastEndedAt !== null && streamer.lastStartedAt !== null
      ? `Last live ${formatRelative(streamer.lastEndedAt)} for ${formatDuration(
          streamer.lastEndedAt - streamer.lastStartedAt,
        )}${
          streamer.lastMaxViewerCount
            ? `, peak ${formatCompactNumber(streamer.lastMaxViewerCount)} viewers`
            : ""
        }`
      : "No streams seen yet";

  return (
    <a
      className="offline-pill"
      href={url}
      target="_blank"
      rel="noreferrer"
      title={title}
    >
      {streamer.bindings[0] && (
        <PlatformIcon platform={streamer.bindings[0].platform} size={12} />
      )}
      <span className="offline-name">{streamer.displayName}</span>
      {lastLive && <span className="offline-when">{lastLive}</span>}
    </a>
  );
}

export function LiveNow({ streamers }: { streamers: StreamerView[] }) {
  if (streamers.length === 0) return null;

  const live = streamers
    .filter((s): s is LiveStreamer => s.live)
    .sort((a, b) => b.maxViewerCount - a.maxViewerCount);
  const offline = streamers
    .filter((s): s is OfflineStreamer => !s.live)
    .sort((a, b) => (b.lastEndedAt ?? 0) - (a.lastEndedAt ?? 0));

  return (
    <section className="page-section">
      <h2 className="section-title">
        Channels
        {live.length > 0 && (
          <span className="section-count live-count">{live.length} live</span>
        )}
      </h2>
      {live.length > 0 && (
        <div className="live-grid">
          {live.map((s) => (
            <LiveStreamerCard key={s.id} streamer={s} />
          ))}
        </div>
      )}
      {offline.length > 0 && (
        <div className="offline-strip">
          {offline.map((s) => (
            <OfflinePill key={s.id} streamer={s} />
          ))}
        </div>
      )}
    </section>
  );
}
