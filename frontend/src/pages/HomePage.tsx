import { useEffect, useState } from "react";
import { fetchTaskRuns } from "../api";
import type { TaskInfo, TaskRun, TaskTrigger } from "../api";
import { Toast, useToast } from "../components/Toast";
import { useTasks } from "../hooks/useTasks";
import { Link } from "../router";
import { describeCron } from "../utils/cron";
import {
  formatAbsolute,
  formatDuration,
  formatRelative,
} from "../utils/format";

const TRIGGER_LABELS: Record<TaskTrigger, string> = {
  schedule: "schedule",
  manual: "manual",
  startup: "startup",
};

function TriggerBadge({ trigger }: { trigger: TaskTrigger }) {
  return (
    <span className={`trigger-badge trigger-${trigger}`}>
      {TRIGGER_LABELS[trigger]}
    </span>
  );
}

function StatusDot({ status }: { status: TaskRun["status"] }) {
  return <span className={`status-dot status-${status}`} />;
}

function runDuration(run: TaskRun): string {
  const end = run.finishedAt ?? Date.now();
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
      {!run.error && run.summary && (
        <div className="run-summary">{run.summary}</div>
      )}
    </div>
  );
}

function TaskCard({
  task,
  onRun,
}: {
  task: TaskInfo;
  onRun: (name: string) => void;
}) {
  const human = describeCron(task.schedule);
  const nextRunIso = task.nextRuns[0];
  const nextRunMs = nextRunIso ? new Date(nextRunIso).getTime() : null;

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
          <span>
            {formatRelative(nextRunMs)}
            <span className="muted"> &middot; {formatAbsolute(nextRunMs)}</span>
          </span>
        ) : (
          <span className="muted">not scheduled</span>
        )}
      </div>
      <LastRunSummary run={task.lastRun} />
    </div>
  );
}

function RecentActivity({ runs }: { runs: TaskRun[] }) {
  if (runs.length === 0) {
    return <div className="muted activity-empty">No task runs recorded yet.</div>;
  }
  return (
    <div className="activity-list">
      {runs.map((run) => (
        <div key={run.runId} className="activity-row">
          <StatusDot status={run.status} />
          <span className="activity-task">{run.taskName}</span>
          <TriggerBadge trigger={run.trigger} />
          <span
            className="activity-time"
            title={formatAbsolute(run.startedAt)}
          >
            {formatRelative(run.startedAt)}
          </span>
          <span className="activity-duration muted">{runDuration(run)}</span>
          {(run.error || run.summary) && (
            <span
              className={`activity-detail ${run.error ? "run-error" : "run-summary"}`}
            >
              {run.error ?? run.summary}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

export default function HomePage() {
  const { tasks, loading, error, runTask } = useTasks();
  const { toast, showToast } = useToast();
  const [runs, setRuns] = useState<TaskRun[] | null>(null);
  const [runsError, setRunsError] = useState<string | null>(null);

  // Refresh recent activity every time the task poll produces new data,
  // so finished runs show up without a separate poll loop.
  useEffect(() => {
    if (tasks === null) return;
    let cancelled = false;
    fetchTaskRuns({ limit: 20 })
      .then((data) => {
        if (cancelled) return;
        setRuns(data.runs);
        setRunsError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setRunsError(
          err instanceof Error ? err.message : "Failed to fetch task runs",
        );
      });
    return () => {
      cancelled = true;
    };
  }, [tasks]);

  const handleRun = async (name: string) => {
    const result = await runTask(name);
    showToast(result.message, result.ok ? "info" : "error");
  };

  return (
    <>
      <h1>omni-notify</h1>
      <Toast toast={toast} />

      <section className="page-section">
        <h2 className="section-title">Tasks</h2>
        {loading && <div className="loading-inline">Loading tasks…</div>}
        {error && tasks === null && (
          <div className="error-inline">Failed to load tasks: {error}</div>
        )}
        {tasks !== null && (
          <>
            {error && (
              <div className="error-inline stale-note">
                Refresh failed ({error}) — showing last known state.
              </div>
            )}
            <div className="task-grid">
              {tasks.map((task) => (
                <TaskCard key={task.name} task={task} onRun={handleRun} />
              ))}
            </div>
            {tasks.length === 0 && (
              <div className="muted">No scheduled tasks registered.</div>
            )}
          </>
        )}
      </section>

      <section className="page-section">
        <h2 className="section-title">Recent activity</h2>
        {runs === null && runsError === null && (
          <div className="loading-inline">Loading activity…</div>
        )}
        {runsError && runs === null && (
          <div className="error-inline">
            Failed to load activity: {runsError}
          </div>
        )}
        {runs !== null && <RecentActivity runs={runs} />}
      </section>

      <section className="page-section">
        <div className="home-nav-cards">
          <Link to="/pets" className="home-nav-card">
            <span className="home-nav-title">Pet Weight Tracker</span>
            <span className="home-nav-desc">
              Weight and litter-visit charts for the pets.
            </span>
          </Link>
          <Link to="/recommendations" className="home-nav-card">
            <span className="home-nav-title">Recommendations</span>
            <span className="home-nav-desc">
              AI-picked movies and shows, with watchlist status.
            </span>
          </Link>
        </div>
      </section>
    </>
  );
}
