import type { LiveStreamer, OfflineStreamer, StreamerView } from "../api";
import { useNow } from "../hooks/useNow";
import { Link } from "../router";
import {
  formatCompactNumber,
  formatDuration,
  formatRelative,
  formatUptime,
} from "../utils/format";
import { PlatformIcon } from "./PlatformIcon";

function streamerPath(id: string): string {
  return `/streamers/${encodeURIComponent(id)}`;
}

function platformLabel(platform: string): string {
  return platform.length > 0 ? platform[0].toUpperCase() + platform.slice(1) : platform;
}

/** Current viewers when known, else "peak" from this session's high-water mark. */
function viewersLabel(streamer: LiveStreamer): string | null {
  if (streamer.viewerCount !== null) {
    return `${formatCompactNumber(streamer.viewerCount)} watching`;
  }
  if (streamer.maxViewerCount > 0) {
    return `${formatCompactNumber(streamer.maxViewerCount)} peak`;
  }
  return null;
}

function LiveStreamerCard({ streamer }: { streamer: LiveStreamer }) {
  const now = useNow(1000);
  const viewers = viewersLabel(streamer);

  return (
    <div className="live-card">
      <div className="live-card-header">
        <PlatformIcon platform={streamer.primary.platform} size={16} />
        <span className="live-name">{streamer.displayName}</span>
        <span className="live-badge">
          <span className="live-badge-dot" />
          LIVE
        </span>
      </div>
      <div className="live-title">{streamer.title}</div>
      <div className="meta-row live-meta">
        <span className="live-uptime">{formatUptime(now - streamer.startedAt)}</span>
        {viewers && <span>{viewers}</span>}
        {streamer.category && <span>{streamer.category}</span>}
      </div>
      {/* Stretched-link pattern: the whole card opens the stream; the compact
          details control above it routes to the streamer detail page. Two
          real anchors, never nested, so mobile long-press/open-in-app still
          works on the primary action. */}
      <a
        className="live-card-overlay"
        href={streamer.primary.url}
        target="_blank"
        rel="noopener"
        aria-label={`Open stream on ${platformLabel(streamer.primary.platform)}`}
      />
      <Link
        className="live-card-details"
        to={streamerPath(streamer.id)}
        ariaLabel={`View details for ${streamer.displayName}`}
      >
        <span aria-hidden="true">Details ›</span>
      </Link>
    </div>
  );
}

function OfflinePill({ streamer }: { streamer: OfflineStreamer }) {
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
    <Link className="offline-pill" to={streamerPath(streamer.id)} title={title}>
      {streamer.bindings[0] && (
        <PlatformIcon platform={streamer.bindings[0].platform} size={12} />
      )}
      <span className="offline-name">{streamer.displayName}</span>
      {lastLive && <span className="offline-when">{lastLive}</span>}
    </Link>
  );
}

export function LiveNow({ streamers }: { streamers: StreamerView[] }) {
  if (streamers.length === 0) return null;

  // The snapshot and iOS live-slot API share one server-side ordering
  // primitive, so every client displays the exact same rank without copying
  // business rules into its presentation layer.
  const live = streamers.filter((s): s is LiveStreamer => s.live);
  const offline = streamers.filter((s): s is OfflineStreamer => !s.live);

  return (
    <section className="page-section live-now-section">
      <h2 className="section-title">
        Live Now
        {live.length > 0 && (
          <span className="section-count live-count">{live.length} live</span>
        )}
      </h2>
      {live.length === 0 ? (
        <div className="muted live-empty">No one is live right now.</div>
      ) : (
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
