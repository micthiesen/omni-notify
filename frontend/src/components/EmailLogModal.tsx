import { useEffect, useState } from "react";
import { fetchEmailActivityLogs } from "../api";
import type { EmailActivity, RunLogLine } from "../api";
import { formatAbsolute } from "../utils/format";
import { OUTCOME_LABELS, PIPELINE_LABELS } from "../utils/emailLabels";
import { LogLines } from "./LogViewer";

/**
 * Modal showing the captured log lines for one email's processing phase.
 * Filtered/skipped emails never reach processing, so they legitimately have
 * no lines; the activity's detail line still explains the outcome.
 */
export function EmailLogModal({
  activity,
  onClose,
}: {
  activity: EmailActivity;
  onClose: () => void;
}) {
  const [lines, setLines] = useState<RunLogLine[] | null>(null);
  const [dropped, setDropped] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchEmailActivityLogs(activity.activityId)
      .then((data) => {
        if (cancelled) return;
        setLines(data.lines);
        setDropped(data.dropped);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "Failed to fetch logs");
      });
    return () => {
      cancelled = true;
    };
  }, [activity.activityId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.body.style.overflow = "";
    };
  }, [onClose]);

  return (
    <div className="modal-root">
      <button
        type="button"
        className="modal-backdrop"
        onClick={onClose}
        aria-label="Close log viewer"
      />
      <div
        className="log-modal"
        role="dialog"
        aria-label={`Logs for ${activity.subject || "email"}`}
      >
        <div className="log-modal-header">
          <div className="log-modal-title">
            <span className={`mail-outcome mail-outcome-${activity.outcome}`}>
              {OUTCOME_LABELS[activity.outcome]}
            </span>
            <span className="log-modal-task email-log-subject">
              {activity.subject || "(no subject)"}
            </span>
          </div>
          <div className="log-modal-meta meta-row muted">
            <span>{PIPELINE_LABELS[activity.pipeline]}</span>
            <span className="email-log-from">{activity.from}</span>
            <span>{formatAbsolute(activity.processedAt)}</span>
          </div>
          <button
            type="button"
            className="log-modal-close"
            onClick={onClose}
            aria-label="Close"
          >
            ✕
          </button>
        </div>
        {activity.detail && (
          <div className="muted log-modal-error">{activity.detail}</div>
        )}
        <div className="log-modal-body">
          {error && <div className="error-inline">Failed to load logs: {error}</div>}
          {!error && lines === null && (
            <div className="loading-inline">Loading logs…</div>
          )}
          {!error && lines !== null && lines.length === 0 && (
            <div className="muted log-empty">
              No processing logs for this email — it never reached extraction.
            </div>
          )}
          {lines !== null && lines.length > 0 && <LogLines lines={lines} />}
        </div>
        {dropped > 0 && (
          <div className="log-modal-footer muted">{dropped} oldest lines dropped</div>
        )}
      </div>
    </div>
  );
}
