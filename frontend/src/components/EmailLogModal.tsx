import { useEffect, useState } from "react";
import {
  createEmailRule,
  fetchEmailActivityLogs,
  forgetParcelDelivery,
  reprocessEmailActivity,
  sendEmailActivityFeedback,
} from "../api";
import type {
  EmailActivity,
  EmailFeedback,
  EmailFeedbackVerdict,
  EmailPipeline,
  RunLogLine,
} from "../api";
import { formatAbsolute } from "../utils/format";
import { OUTCOME_LABELS, PIPELINE_LABELS } from "../utils/emailLabels";
import { LogLines } from "./LogViewer";

const FEEDBACK_LABELS: Record<
  EmailPipeline,
  Record<EmailFeedbackVerdict, string>
> = {
  ParcelTracker: { not_relevant: "Not a parcel", missed: "Missed parcel" },
  CalendarEvents: { not_relevant: "Not an event", missed: "Missed event" },
};

const BLOCK_SCOPE: Record<EmailPipeline, "parcel" | "calendar"> = {
  ParcelTracker: "parcel",
  CalendarEvents: "calendar",
};

const BLOCK_SCOPE_LABELS: Record<EmailPipeline, string> = {
  ParcelTracker: "Parcels",
  CalendarEvents: "Calendar",
};

/**
 * Item lines look like "<token> (...)..." or "<token>: ..."; the leading token
 * is the tracking number. Returns null when the line has neither shape.
 */
function extractTrackingNumber(item: string): string | null {
  const match = /^(\S+?)(?::|\s+\()/.exec(item);
  return match ? match[1] : null;
}

/**
 * Modal showing the captured log lines for one email's processing phase, plus
 * actions: reprocess, feedback, block sender, and per-item parcel "forget".
 * Filtered/skipped emails never reach processing, so they legitimately have
 * no lines; the activity's detail line still explains the outcome.
 */
export function EmailLogModal({
  activity,
  feedback,
  onActivityChange,
  onFeedbackChange,
  onClose,
}: {
  activity: EmailActivity;
  feedback?: EmailFeedback | null;
  onActivityChange?: (activity: EmailActivity) => void;
  onFeedbackChange?: (activityId: string, feedback: EmailFeedback | null) => void;
  onClose: () => void;
}) {
  const [current, setCurrent] = useState(activity);
  const [lines, setLines] = useState<RunLogLine[] | null>(null);
  const [dropped, setDropped] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [logsVersion, setLogsVersion] = useState(0);

  const [reprocessing, setReprocessing] = useState(false);
  const [fb, setFb] = useState<EmailFeedback | null>(feedback ?? null);
  const [fbBusy, setFbBusy] = useState(false);
  const [blocking, setBlocking] = useState(false);
  const [blockNote, setBlockNote] = useState<string | null>(null);
  const [forgetting, setForgetting] = useState<string | null>(null);
  const [forgotten, setForgotten] = useState<ReadonlySet<string>>(new Set());
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLines(null);
    fetchEmailActivityLogs(current.activityId)
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
  }, [current.activityId, logsVersion]);

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

  const handleReprocess = async () => {
    if (reprocessing) return;
    setReprocessing(true);
    setActionError(null);
    setBlockNote(null);
    try {
      const res = await reprocessEmailActivity(current.activityId);
      setCurrent(res.activity);
      onActivityChange?.(res.activity);
      // The rerun may have resubmitted a forgotten number; stale strikethrough
      // state would hide the Forget button for a now-live submission.
      setForgotten(new Set());
      setLogsVersion((v) => v + 1);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Reprocess failed");
    } finally {
      setReprocessing(false);
    }
  };

  const handleFeedback = async (verdict: EmailFeedbackVerdict) => {
    if (fbBusy) return;
    const next = fb?.verdict === verdict ? null : verdict;
    setFbBusy(true);
    setActionError(null);
    try {
      const res = await sendEmailActivityFeedback(current.activityId, next);
      setFb(res.feedback);
      onFeedbackChange?.(current.activityId, res.feedback);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Feedback failed");
    } finally {
      setFbBusy(false);
    }
  };

  const handleBlockSender = async () => {
    if (blocking) return;
    const pattern = current.from.toLowerCase();
    const scopeLabel = BLOCK_SCOPE_LABELS[current.pipeline];
    const confirmed = window.confirm(
      `Block ${pattern} for ${scopeLabel}? Future emails from this sender will be filtered.`,
    );
    if (!confirmed) return;
    setBlocking(true);
    setActionError(null);
    setBlockNote(null);
    try {
      await createEmailRule({
        pattern,
        scope: BLOCK_SCOPE[current.pipeline],
        verdict: "block",
      });
      setBlockNote(`Blocked ${pattern} for ${scopeLabel}`);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Block failed");
    } finally {
      setBlocking(false);
    }
  };

  const handleForget = async (trackingNumber: string) => {
    if (forgetting !== null) return;
    const confirmed = window.confirm(
      `Forget tracking number ${trackingNumber}? A future email will be able to resubmit it.`,
    );
    if (!confirmed) return;
    setForgetting(trackingNumber);
    setActionError(null);
    try {
      await forgetParcelDelivery(trackingNumber);
      setForgotten((prev) => new Set(prev).add(trackingNumber));
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Forget failed");
    } finally {
      setForgetting(null);
    }
  };

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
        aria-label={`Logs for ${current.subject || "email"}`}
      >
        <div className="log-modal-header">
          <div className="log-modal-title">
            <span className={`mail-outcome mail-outcome-${current.outcome}`}>
              {OUTCOME_LABELS[current.outcome]}
            </span>
            <span className="log-modal-task email-log-subject">
              {current.subject || "(no subject)"}
            </span>
          </div>
          <div className="log-modal-meta meta-row muted">
            <span>{PIPELINE_LABELS[current.pipeline]}</span>
            <span className="email-log-from">{current.from}</span>
            <span>{formatAbsolute(current.processedAt)}</span>
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
        {current.detail && (
          <div className="muted log-modal-error">{current.detail}</div>
        )}
        {current.admitReason && (
          <div className="muted email-admit-line">
            Admitted: {current.admitReason}
          </div>
        )}
        {current.items.length > 0 && (
          <ul className="email-modal-items">
            {current.items.map((item, index) => {
              const trackingNumber =
                current.pipeline === "ParcelTracker"
                  ? extractTrackingNumber(item)
                  : null;
              const isForgotten =
                trackingNumber !== null && forgotten.has(trackingNumber);
              return (
                <li key={`${index}-${item}`}>
                  <span
                    className={isForgotten ? "email-item-forgotten" : undefined}
                  >
                    {item}
                  </span>
                  {isForgotten && (
                    <span className="email-item-note muted">Forgotten</span>
                  )}
                  {trackingNumber !== null && !isForgotten && (
                    <button
                      type="button"
                      className="email-item-forget"
                      disabled={forgetting !== null}
                      onClick={() => handleForget(trackingNumber)}
                    >
                      {forgetting === trackingNumber ? "Forgetting…" : "Forget"}
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        <div className="email-actions">
          <button
            type="button"
            className="run-btn"
            disabled={reprocessing}
            onClick={handleReprocess}
          >
            {reprocessing && <span className="spinner" aria-hidden="true" />}
            {reprocessing ? "Reprocessing…" : "Reprocess"}
          </button>
          {(["not_relevant", "missed"] as const).map((verdict) => (
            <button
              key={verdict}
              type="button"
              className={`chip-btn ${fb?.verdict === verdict ? "active" : ""}`}
              disabled={fbBusy}
              onClick={() => handleFeedback(verdict)}
            >
              {FEEDBACK_LABELS[current.pipeline][verdict]}
            </button>
          ))}
          <button
            type="button"
            className="email-block-btn"
            disabled={blocking}
            onClick={handleBlockSender}
          >
            {blocking ? "Blocking…" : "Block sender"}
          </button>
        </div>
        {blockNote && <div className="email-action-note muted">{blockNote}</div>}
        {actionError && (
          <div className="email-action-error error-inline">{actionError}</div>
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
