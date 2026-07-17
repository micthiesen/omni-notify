import { useEffect, useState } from "react";
import { fetchTaskRuns } from "../api";
import type { TaskInfo, TaskRun } from "../api";
import { useNow } from "../hooks/useNow";
import { describeCron } from "../utils/cron";
import {
  formatAbsolute,
  formatCountdown,
  formatDuration,
  formatRelative,
  toTitleCase,
} from "../utils/format";
import { StatusDot, TriggerBadge } from "./badges";

const HISTORY_LIMIT = 10;

export function runDuration(run: TaskRun, now = Date.now()): string {
  const end = run.finishedAt ?? now;
  return formatDuration(end - run.startedAt);
}

function LastRunSummary({
  run,
  onViewLogs,
}: {
  run: TaskRun | null;
  onViewLogs: (run: TaskRun) => void;
}) {
  if (!run) {
    return <div className="task-last-run muted">No runs yet</div>;
  }
  return (
    <button
      type="button"
      className="task-last-run row-btn"
      onClick={() => onViewLogs(run)}
      title="View logs"
    >
      <div className="task-last-run-meta">
        <StatusDot status={run.status} />
        <span className={`run-status-text run-status-${run.status}`}>
          {run.status}
        </span>
        <span title={formatAbsolute(run.startedAt)}>
          {formatRelative(run.startedAt)}
        </span>
        <span className="muted">{runDuration(run)}</span>
        <TriggerBadge trigger={run.trigger} />
      </div>
      {run.error && <div className="run-error">{run.error}</div>}
      {!run.error && run.summary && <div className="run-summary">{run.summary}</div>}
    </button>
  );
}

function HistoryList({
  runs,
  onViewLogs,
}: {
  runs: TaskRun[];
  onViewLogs: (run: TaskRun) => void;
}) {
  if (runs.length === 0) {
    return <div className="muted history-empty">No earlier runs.</div>;
  }
  return (
    <div className="history-list">
      {runs.map((run) => (
        <button
          key={run.runId}
          type="button"
          className="history-row row-btn"
          onClick={() => onViewLogs(run)}
          title="View logs"
        >
          <StatusDot status={run.status} />
          <span className="history-time" title={formatAbsolute(run.startedAt)}>
            {formatRelative(run.startedAt)}
          </span>
          <span className="muted">{runDuration(run)}</span>
          <TriggerBadge trigger={run.trigger} />
          {(run.error || run.summary) && (
            <span
              className={`history-detail ${run.error ? "run-error" : "run-summary"}`}
            >
              {run.error ?? run.summary}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}

export function TaskCard({
  task,
  onRun,
  onViewLogs,
}: {
  task: TaskInfo;
  onRun: (name: string) => void;
  onViewLogs: (run: TaskRun) => void;
}) {
  const now = useNow(1000);
  const [expanded, setExpanded] = useState(false);
  const [history, setHistory] = useState<TaskRun[] | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);

  const human = describeCron(task.schedule);
  const nextRunIso = task.nextRuns[0];
  const nextRunMs = nextRunIso ? new Date(nextRunIso).getTime() : null;
  const lastRunId = task.lastRun?.runId ?? null;

  // Refetch whenever a new run appears or the current one finishes, so the
  // open panel tracks reality without its own poll loop.
  const lastRunFinished = task.lastRun?.finishedAt ?? null;
  useEffect(() => {
    if (!expanded) return;
    let cancelled = false;
    // Fetch one extra: the newest run already shows in the card's last-run
    // line, so it's excluded from the accordion below.
    fetchTaskRuns({ task: task.name, limit: HISTORY_LIMIT + 1 })
      .then((data) => {
        if (cancelled) return;
        setHistory(data.runs);
        setHistoryError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setHistoryError(
          err instanceof Error ? err.message : "Failed to fetch run history",
        );
      });
    return () => {
      cancelled = true;
    };
  }, [expanded, task.name, lastRunId, lastRunFinished]);

  return (
    <div className="task-card">
      <div className="task-card-header">
        <div className="task-name-wrap">
          {task.running ? (
            <span className="running-pulse" title="Running" />
          ) : (
            <span
              className={`status-dot status-${task.lastRun?.status ?? "none"}`}
              title={
                task.lastRun ? `Last run: ${task.lastRun.status}` : "No runs yet"
              }
            />
          )}
          <span className="task-name">{toTitleCase(task.name)}</span>
        </div>
        <button
          type="button"
          className="run-btn"
          disabled={task.running}
          onClick={() => onRun(task.name)}
        >
          {task.running ? "Running…" : "Run now"}
        </button>
      </div>
      <div className="task-schedule">
        <code className="cron-raw">{task.schedule}</code>
        {human && <span className="cron-human">{human}</span>}
      </div>
      <div className="task-next-run">
        <span className="field-label">Next run</span>
        {nextRunMs !== null && !Number.isNaN(nextRunMs) ? (
          <span
            className="next-run-value meta-row"
            title={formatAbsolute(nextRunMs)}
          >
            <span>{formatCountdown(nextRunMs - now)}</span>
            <span className="muted">{formatAbsolute(nextRunMs)}</span>
          </span>
        ) : (
          <span className="muted">Not scheduled</span>
        )}
      </div>
      <LastRunSummary run={task.lastRun} onViewLogs={onViewLogs} />
      {expanded && (
        <div className="task-history">
          {history === null && historyError === null && (
            <div className="muted history-empty">Loading…</div>
          )}
          {historyError && (
            <div className="error-inline history-empty">{historyError}</div>
          )}
          {history !== null && (
            <HistoryList
              runs={history
                .filter((run) => run.runId !== lastRunId)
                .slice(0, HISTORY_LIMIT)}
              onViewLogs={onViewLogs}
            />
          )}
        </div>
      )}
      <button
        type="button"
        className="history-toggle"
        onClick={() => setExpanded((v) => !v)}
      >
        {expanded ? "Hide history" : "History"}
        <span className={`chevron ${expanded ? "open" : ""}`}>▾</span>
      </button>
    </div>
  );
}
