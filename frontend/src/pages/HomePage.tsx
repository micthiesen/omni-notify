import { useMemo, useState } from "react";
import type { TaskInfo, TaskRun } from "../api";
import { ActivityFeed } from "../components/ActivityFeed";
import { LiveNow } from "../components/LiveNow";
import { LogViewer } from "../components/LogViewer";
import { StatStrip } from "../components/StatStrip";
import { TaskCard } from "../components/TaskCard";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";

/** Soonest next run first; tasks with no scheduled next run sort last. */
function nextRunMs(task: TaskInfo): number {
  const iso = task.nextRuns[0];
  if (!iso) return Number.POSITIVE_INFINITY;
  const ms = new Date(iso).getTime();
  return Number.isNaN(ms) ? Number.POSITIVE_INFINITY : ms;
}

function sortTasksByNextRun(tasks: TaskInfo[]): TaskInfo[] {
  return [...tasks].sort((a, b) => nextRunMs(a) - nextRunMs(b));
}

export default function HomePage() {
  const { snapshot, error, runTask } = useLiveData();
  const { toast, showToast } = useToast();
  const [logRun, setLogRun] = useState<TaskRun | null>(null);

  const handleRun = async (name: string) => {
    const result = await runTask(name);
    showToast(result.message, result.ok ? "info" : "error");
  };

  const sortedTasks = useMemo(
    () => (snapshot ? sortTasksByNextRun(snapshot.tasks) : []),
    [snapshot],
  );

  if (snapshot === null) {
    return error !== null ? (
      <div className="error">
        <div>Failed to load dashboard</div>
        <div className="error-detail">{error}</div>
      </div>
    ) : (
      <div className="loading">Loading…</div>
    );
  }

  return (
    <>
      <Toast toast={toast} />
      {error && (
        <div className="error-inline stale-note">
          Refresh failed ({error}) — showing last known state.
        </div>
      )}

      <StatStrip snapshot={snapshot} />

      <LiveNow streamers={snapshot.streamers} />

      <section className="page-section">
        <h2 className="section-title">Tasks</h2>
        {snapshot.tasks.length === 0 ? (
          <div className="muted">No scheduled tasks registered.</div>
        ) : (
          <div className="task-grid">
            {sortedTasks.map((task) => (
              <TaskCard
                key={task.name}
                task={task}
                onRun={handleRun}
                onViewLogs={setLogRun}
              />
            ))}
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
