import { useEffect, useMemo, useState } from "react";
import { fetchTaskRuns } from "../api";
import type { Snapshot, TaskRun } from "../api";
import { formatAbsolute, formatRelative } from "../utils/format";
import { StatusDot, TriggerBadge } from "./badges";
import { runDuration } from "./TaskCard";

const FILTERED_LIMIT = 100;

/**
 * Consecutive successful runs of the same task collapse into one row (the
 * 20-second LiveCheck cadence would otherwise drown everything else).
 * Errors and in-flight runs always get their own row.
 */
interface ActivityGroup {
  key: string;
  runs: TaskRun[]; // newest first
}

export function groupRuns(runs: TaskRun[]): ActivityGroup[] {
  const groups: ActivityGroup[] = [];
  for (const run of runs) {
    const last = groups[groups.length - 1];
    if (
      last &&
      run.status === "success" &&
      last.runs[0].status === "success" &&
      last.runs[0].taskName === run.taskName
    ) {
      last.runs.push(run);
    } else {
      groups.push({ key: run.runId, runs: [run] });
    }
  }
  return groups;
}

function GroupRow({
  group,
  onViewLogs,
}: {
  group: ActivityGroup;
  onViewLogs: (run: TaskRun) => void;
}) {
  const newest = group.runs[0];
  const oldest = group.runs[group.runs.length - 1];
  const count = group.runs.length;
  return (
    <button
      type="button"
      className="activity-row row-btn"
      onClick={() => onViewLogs(newest)}
      title="View logs"
    >
      <StatusDot status={newest.status} />
      <span className="activity-task">{newest.taskName}</span>
      {count > 1 && (
        <span
          className="collapse-badge"
          title={`${count} consecutive runs, oldest ${formatRelative(oldest.startedAt)}`}
        >
          ×{count}
        </span>
      )}
      <TriggerBadge trigger={newest.trigger} />
      <span className="activity-time" title={formatAbsolute(newest.startedAt)}>
        {formatRelative(newest.startedAt)}
      </span>
      <span className="activity-duration muted">{runDuration(newest)}</span>
      {(newest.error || newest.summary) && (
        <span
          className={`activity-detail ${newest.error ? "run-error" : "run-summary"}`}
        >
          {newest.error ?? newest.summary}
        </span>
      )}
    </button>
  );
}

export function ActivityFeed({
  snapshot,
  onViewLogs,
}: {
  snapshot: Snapshot;
  onViewLogs: (run: TaskRun) => void;
}) {
  const [filterTask, setFilterTask] = useState("");
  const [errorsOnly, setErrorsOnly] = useState(false);
  const [fetched, setFetched] = useState<TaskRun[] | null>(null);
  const [fetchError, setFetchError] = useState<string | null>(null);

  const filtered = filterTask !== "" || errorsOnly;
  const newestRunId = snapshot.runs[0]?.runId ?? null;

  // Filtered views need deeper history than the snapshot carries; refetch
  // whenever new runs land so the view stays live.
  useEffect(() => {
    if (!filtered) {
      setFetched(null);
      setFetchError(null);
      return;
    }
    let cancelled = false;
    fetchTaskRuns({ task: filterTask || undefined, limit: FILTERED_LIMIT })
      .then((data) => {
        if (cancelled) return;
        setFetched(data.runs);
        setFetchError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setFetchError(
          err instanceof Error ? err.message : "Failed to fetch task runs",
        );
      });
    return () => {
      cancelled = true;
    };
  }, [filtered, filterTask, newestRunId]);

  const runs = filtered ? fetched : snapshot.runs;
  const visible = useMemo(() => {
    if (runs === null) return null;
    const base = errorsOnly ? runs.filter((r) => r.status === "error") : runs;
    return groupRuns(base);
  }, [runs, errorsOnly]);

  const taskNames = snapshot.tasks.map((t) => t.name);

  return (
    <>
      <div className="activity-controls">
        <select
          className="activity-filter"
          value={filterTask}
          onChange={(e) => setFilterTask(e.target.value)}
        >
          <option value="">All tasks</option>
          {taskNames.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
        <button
          type="button"
          className={`chip-btn ${errorsOnly ? "active" : ""}`}
          onClick={() => setErrorsOnly((v) => !v)}
        >
          Errors only
        </button>
      </div>
      {filtered && fetchError && fetched === null && (
        <div className="error-inline">Failed to load activity: {fetchError}</div>
      )}
      {visible === null && fetchError === null && (
        <div className="loading-inline">Loading activity…</div>
      )}
      {visible !== null && visible.length === 0 && (
        <div className="muted activity-empty">
          {errorsOnly ? "No errors recorded. 🎉" : "No task runs recorded yet."}
        </div>
      )}
      {visible !== null && visible.length > 0 && (
        <div className="activity-list">
          {visible.map((group) => (
            <GroupRow key={group.key} group={group} onViewLogs={onViewLogs} />
          ))}
        </div>
      )}
    </>
  );
}
