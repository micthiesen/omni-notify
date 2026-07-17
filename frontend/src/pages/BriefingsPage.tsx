import { useEffect, useMemo, useState } from "react";
import { type BriefingHistory, fetchBriefings } from "../api";
import { ShowMoreButton, useShowMore } from "../components/ShowMore";
import { formatAbsoluteWithYear, toTitleCase } from "../utils/format";

interface FeedEntry {
  briefingName: string;
  title: string;
  message: string;
  url: string;
  timestamp: number;
}

function buildFeed(briefings: BriefingHistory[], filter: string | null): FeedEntry[] {
  return briefings
    .filter((b) => filter === null || b.name === filter)
    .flatMap((b) =>
      b.notifications.map((n) => ({ briefingName: b.name, ...n })),
    )
    .sort((a, b) => b.timestamp - a.timestamp);
}

export default function BriefingsPage() {
  const [briefings, setBriefings] = useState<BriefingHistory[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchBriefings()
      .then((res) => {
        if (!cancelled) setBriefings(res.briefings);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load briefings");
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const feed = useMemo(
    () => (briefings ? buildFeed(briefings, filter) : []),
    [briefings, filter],
  );
  const { visible, hasMore, remaining, showMore } = useShowMore(feed, 20, filter);

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>Briefings</h1>
          <p className="page-subtitle">
            Archive of AI briefing notifications (last 50 per briefing).
          </p>
        </div>
      </div>

      {briefings === null && error === null && (
        <div className="loading">Loading…</div>
      )}
      {error && briefings === null && (
        <div className="error">
          <div>Failed to load briefings</div>
          <div className="error-detail">{error}</div>
        </div>
      )}

      {briefings !== null && briefings.length === 0 && (
        <div className="muted">No briefings have run yet.</div>
      )}

      {briefings !== null && briefings.length > 0 && (
        <>
          <div className="rec-filters">
            <button
              type="button"
              className={`chip-btn ${filter === null ? "active" : ""}`}
              onClick={() => setFilter(null)}
            >
              All
            </button>
            {briefings.map((b) => (
              <button
                key={b.name}
                type="button"
                className={`chip-btn ${filter === b.name ? "active" : ""}`}
                onClick={() => setFilter(filter === b.name ? null : b.name)}
              >
                {toTitleCase(b.name)}
                <span className="chip-btn-count">{b.notifications.length}</span>
              </button>
            ))}
          </div>

          <div className="briefing-feed">
            {visible.map((entry, index) => (
              <article
                key={`${index}-${entry.briefingName}-${entry.timestamp}`}
                className="briefing-card"
              >
                <div className="briefing-card-header">
                  <h2 className="briefing-card-title">{entry.title}</h2>
                  <span className="briefing-card-time">
                    {formatAbsoluteWithYear(entry.timestamp)}
                  </span>
                </div>
                <div className="briefing-card-meta">
                  <span className="briefing-badge">
                    {toTitleCase(entry.briefingName)}
                  </span>
                  {entry.url && (
                    <a
                      href={entry.url}
                      target="_blank"
                      rel="noreferrer"
                      className="briefing-source"
                    >
                      Source ↗
                    </a>
                  )}
                </div>
                <p className="briefing-message">{entry.message}</p>
              </article>
            ))}
            {feed.length === 0 && (
              <div className="muted">No notifications for this briefing yet.</div>
            )}
          </div>
          {hasMore && <ShowMoreButton remaining={remaining} onClick={showMore} />}
        </>
      )}
    </>
  );
}
