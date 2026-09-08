import { useMemo, useState } from "react";
import type { TaskInfo, TaskRun } from "../api";
import { ActivityFeed } from "../components/ActivityFeed";
import { LogViewer } from "../components/LogViewer";
import { StatStrip } from "../components/StatStrip";
import { TaskCard } from "../components/TaskCard";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";
import { taskLabel } from "../utils/format";

function nextRunMs(task: TaskInfo): number {
  const iso = task.nextRuns[0];
  if (!iso) return Number.POSITIVE_INFINITY;
  const ms = new Date(iso).getTime();
  return Number.isNaN(ms) ? Number.POSITIVE_INFINITY : ms;
}

export default function OperationsPage() {
  const { snapshot, error, runTask } = useLiveData();
  const { toast, showToast } = useToast();
  const [query, setQuery] = useState("");
  const [taskFilter, setTaskFilter] = useState<"all" | "attention" | "running">("all");
  const [logRun, setLogRun] = useState<TaskRun | null>(null);
  const sortedTasks = useMemo(
    () =>
      snapshot
        ? [...snapshot.tasks]
            .filter((task) => {
              const matchesQuery = `${taskLabel(task)} ${task.name}`
                .toLowerCase()
                .includes(query.toLowerCase().trim());
              return (
                matchesQuery &&
                (taskFilter === "all" ||
                  (taskFilter === "running"
                    ? task.running
                    : task.lastRun?.status === "error"))
              );
            })
            .sort((a, b) => {
              const rank = (task: TaskInfo) =>
                task.running ? 0 : task.lastRun?.status === "error" ? 1 : 2;
              return rank(a) - rank(b) || nextRunMs(a) - nextRunMs(b);
            })
        : [],
    [snapshot, query, taskFilter],
  );

  if (!snapshot) {
    return error ? (
      <div className="error">
        <div>Failed to load operations</div>
        <div className="error-detail">{error}</div>
      </div>
    ) : (
      <div className="loading">Loading…</div>
    );
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
          <p className="page-subtitle">
            Task health, controls, run history, and system activity.
          </p>
        </div>
      </div>
      {error && (
        <div className="error-inline stale-note">
          Refresh failed ({error}), showing last known state.
        </div>
      )}
      <StatStrip snapshot={snapshot} />
      <section className="page-section">
        <div className="section-heading-row">
          <h2 className="section-title">
            Tasks <span className="section-count">{snapshot.tasks.length}</span>
          </h2>
        </div>
        <div className="task-toolbar">
          <input
            className="task-search"
            type="search"
            aria-label="Search tasks"
            placeholder="Search tasks…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="task-filters" aria-label="Task status">
            {(
              [
                ["all", "All"],
                ["attention", "Needs Attention"],
                ["running", "Running"],
              ] as const
            ).map(([value, label]) => (
              <button
                key={value}
                type="button"
                className={`chip-btn ${taskFilter === value ? "active" : ""}`}
                aria-pressed={taskFilter === value}
                onClick={() => setTaskFilter(value)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
        {sortedTasks.length === 0 ? (
          <div className="muted">No tasks match this view.</div>
        ) : (
          <div className="task-grid">
            {sortedTasks.map((task) => (
              <TaskCard
                key={task.name}
                task={task}
                onRun={run}
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
