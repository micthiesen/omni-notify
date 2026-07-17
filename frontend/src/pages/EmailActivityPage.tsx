import { useEffect, useState } from "react";
import {
  type EmailActivity,
  type EmailPipeline,
  fetchEmailActivity,
} from "../api";
import { EmailLogModal } from "../components/EmailLogModal";
import { ShowMoreButton, useShowMore } from "../components/ShowMore";
import { OUTCOME_LABELS, PIPELINE_LABELS } from "../utils/emailLabels";
import { formatAbsolute } from "../utils/format";

export default function EmailActivityPage() {
  const [activities, setActivities] = useState<EmailActivity[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pipeline, setPipeline] = useState<EmailPipeline | null>(null);
  const [logsFor, setLogsFor] = useState<EmailActivity | null>(null);

  useEffect(() => {
    let cancelled = false;
    setActivities(null);
    setError(null);
    fetchEmailActivity(pipeline ?? undefined)
      .then((res) => {
        if (!cancelled) setActivities(res.activities);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load activity");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [pipeline]);

  const { visible, hasMore, remaining, showMore } = useShowMore(
    activities ?? [],
    30,
    pipeline,
  );

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>Email activity</h1>
          <p className="page-subtitle">
            What the parcel and calendar pipelines did with each email.
          </p>
        </div>
      </div>

      <div className="rec-filters">
        <button
          type="button"
          className={`chip-btn ${pipeline === null ? "active" : ""}`}
          onClick={() => setPipeline(null)}
        >
          All
        </button>
        {(Object.keys(PIPELINE_LABELS) as EmailPipeline[]).map((p) => (
          <button
            key={p}
            type="button"
            className={`chip-btn ${pipeline === p ? "active" : ""}`}
            onClick={() => setPipeline(pipeline === p ? null : p)}
          >
            {PIPELINE_LABELS[p]}
          </button>
        ))}
      </div>

      {activities === null && error === null && (
        <div className="loading">Loading…</div>
      )}
      {error && activities === null && (
        <div className="error">
          <div>Failed to load email activity</div>
          <div className="error-detail">{error}</div>
        </div>
      )}
      {activities !== null && activities.length === 0 && (
        <div className="muted">
          No email activity recorded yet. Activity appears here as new emails are
          processed.
        </div>
      )}

      {activities !== null && activities.length > 0 && (
        <ul className="mail-list">
          {visible.map((activity) => (
            <li key={activity.activityId} className="mail-row">
              <button
                type="button"
                className="mail-row-btn"
                title="View processing logs"
                onClick={() => setLogsFor(activity)}
              >
                <div className="mail-row-top">
                  <span className="mail-subject" title={activity.subject}>
                    {activity.subject || "(no subject)"}
                  </span>
                  <span className={`mail-outcome mail-outcome-${activity.outcome}`}>
                    {OUTCOME_LABELS[activity.outcome]}
                  </span>
                </div>
                <div className="mail-row-meta">
                  <span className="briefing-badge">
                    {PIPELINE_LABELS[activity.pipeline]}
                  </span>
                  <span className="mail-from" title={activity.from}>
                    {activity.from}
                  </span>
                  <span className="mail-time">
                    {formatAbsolute(activity.processedAt)}
                  </span>
                </div>
                {activity.detail && (
                  <div className="mail-detail">{activity.detail}</div>
                )}
                {activity.items.length > 0 && (
                  <ul className="mail-items">
                    {activity.items.map((item, index) => (
                      <li key={`${index}-${item}`}>{item}</li>
                    ))}
                  </ul>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}
      {hasMore && <ShowMoreButton remaining={remaining} onClick={showMore} />}
      {logsFor && (
        <EmailLogModal activity={logsFor} onClose={() => setLogsFor(null)} />
      )}
    </>
  );
}
