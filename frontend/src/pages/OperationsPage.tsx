import { useMemo, useState } from "react";
import type { TaskInfo, TaskRun } from "../api";
import { ActivityFeed } from "../components/ActivityFeed";
import { LogViewer } from "../components/LogViewer";
import { StatStrip } from "../components/StatStrip";
import { TaskCard } from "../components/TaskCard";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";

function nextRunMs(task: TaskInfo): number {
  const iso = task.nextRuns[0];
  if (!iso) return Number.POSITIVE_INFINITY;
  const ms = new Date(iso).getTime();
  return Number.isNaN(ms) ? Number.POSITIVE_INFINITY : ms;
}

export default function OperationsPage() {
  const { snapshot, error, runTask } = useLiveData();
  const { toast, showToast } = useToast();
  const [logRun, setLogRun] = useState<TaskRun | null>(null);
  const sortedTasks = useMemo(
    () => snapshot ? [...snapshot.tasks].sort((a, b) => nextRunMs(a) - nextRunMs(b)) : [],
    [snapshot],
  );

  if (!snapshot) {
    return error ? (
      <div className="error"><div>Failed to load operations</div><div className="error-detail">{error}</div></div>
    ) : <div className="loading">Loading…</div>;
  }

  const run = async (name: string) => {
    const result = await runTask(name);
    showToast(result.message, result.ok ? "info" : "error");
  };

  return (
    <>
      <Toast toast={toast} />
      <div className="page-header">
        <div className="page-header-stack">
          <h1>Operations</h1>
          <p className="page-subtitle">Task health, controls, run history, and system activity.</p>
        </div>
      </div>
      {error && <div className="error-inline stale-note">Refresh failed ({error}), showing last known state.</div>}
      <StatStrip snapshot={snapshot} />
      <section className="page-section">
        <h2 className="section-title">Tasks</h2>
        {sortedTasks.length === 0 ? <div className="muted">No scheduled tasks registered.</div> : (
          <div className="task-grid">
            {sortedTasks.map((task) => <TaskCard key={task.name} task={task} onRun={run} onViewLogs={setLogRun} />)}
          </div>
        )}
      </section>
      <section className="page-section">
        <h2 className="section-title">Activity</h2>
        <ActivityFeed snapshot={snapshot} onViewLogs={setLogRun} />
      </section>
      {logRun && <LogViewer run={logRun} onClose={() => setLogRun(null)} />}
    </>
  );
}
