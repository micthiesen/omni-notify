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
} from "../utils/format";
import { StatusDot, TriggerBadge } from "./badges";

const HISTORY_LIMIT = 10;

export function runDuration(run: TaskRun, now = Date.now()): string {
  const end = run.finishedAt ?? now;
  return formatDuration(end - run.startedAt);
}

function LastRunSummary({ run }: { run: TaskRun | null }) {
  if (!run) {
    return <div className="task-last-run muted">No runs yet</div>;
  }
  return (
    <div className="task-last-run">
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
    </div>
  );
}

function HistoryList({ runs }: { runs: TaskRun[] }) {
  if (runs.length === 0) {
    return <div className="muted history-empty">No recorded runs.</div>;
  }
  return (
    <div className="history-list">
      {runs.map((run) => (
        <div key={run.runId} className="history-row">
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
        </div>
      ))}
    </div>
  );
}

export function TaskCard({
  task,
  onRun,
}: {
  task: TaskInfo;
  onRun: (name: string) => void;
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
    fetchTaskRuns({ task: task.name, limit: HISTORY_LIMIT })
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
          {task.running && <span className="running-pulse" title="Running" />}
          <span className="task-name">{task.name}</span>
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
          <span className="next-run-value" title={formatAbsolute(nextRunMs)}>
            {formatCountdown(nextRunMs - now)}
            <span className="muted"> &middot; {formatAbsolute(nextRunMs)}</span>
          </span>
        ) : (
          <span className="muted">not scheduled</span>
        )}
      </div>
      <LastRunSummary run={task.lastRun} />
      <button
        type="button"
        className="history-toggle"
        onClick={() => setExpanded((v) => !v)}
      >
        {expanded ? "Hide history" : "History"}
        <span className={`chevron ${expanded ? "open" : ""}`}>▾</span>
      </button>
      {expanded && (
        <div className="task-history">
          {history === null && historyError === null && (
            <div className="muted history-empty">Loading…</div>
          )}
          {historyError && (
            <div className="error-inline history-empty">{historyError}</div>
          )}
          {history !== null && <HistoryList runs={history} />}
        </div>
      )}
    </div>
  );
}
