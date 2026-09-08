import { useState } from "react";
import { Effect } from "effect";
import { fetchTaskRuns } from "../api";
import type { TaskRun } from "../api";
import { useUiEffect } from "../effect";
import { formatAbsolute, formatRelative } from "../utils/format";
import { LogViewer } from "./LogViewer";

function runOutcome(run: TaskRun): { label: string; tone: string } {
  if (run.status === "running") return { label: "Running", tone: "running" };
  if (run.status === "error") return { label: "Error", tone: "error" };
  if (run.summary?.startsWith("no_add:")) {
    return { label: "No Pick", tone: "no-add" };
  }
  return { label: "Completed", tone: "success" };
}

/**
 * Recent runs of a recommendation task, each row clickable to open its logs.
 * Shared by the Media Recommendations and Podcasts pages (parameterized by the
 * task's stable `name`). Refetches whenever `latestRunId` changes so a fresh
 * run appears without a manual refresh.
 */
export function RecommendationRuns({
  taskName,
  latestRunId,
}: {
  taskName: string;
  latestRunId: string | null;
}) {
  const [loadError, setLoadError] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [runs, setRuns] = useState<TaskRun[]>([]);
  const [logRun, setLogRun] = useState<TaskRun | null>(null);

  useUiEffect(
    () =>
      fetchTaskRuns({ task: taskName, limit: 6 }).pipe(
        Effect.tap((data) =>
          Effect.sync(() => {
            setRuns(data.runs);
            setLoaded(true);
            setLoadError(false);
          }),
        ),
        // Recommendation cards remain useful if activity history is unavailable.
        Effect.catch(() =>
          Effect.sync(() => {
            setLoadError(true);
            setLoaded(true);
          }),
        ),
      ),
    [taskName, latestRunId],
  );

  return (
    <>
      <details className="page-section rec-activity-section content-disclosure">
        <summary>Recent Activity</summary>
        {loadError && (
          <div className="error-inline">Activity could not be refreshed.</div>
        )}
        {!loaded ? (
          <div className="muted">Loading activity…</div>
        ) : runs.length === 0 && !loadError ? (
          <div className="muted">No recommendation runs recorded yet.</div>
        ) : (
          <div className="rec-run-list">
            {runs.map((run) => {
              const outcome = runOutcome(run);
              const detail = run.error ?? run.summary;
              return (
                <button
                  type="button"
                  className="rec-run-row row-btn"
                  key={run.runId}
                  onClick={() => setLogRun(run)}
                  title="View logs"
                >
                  <span className={`rec-run-outcome rec-run-${outcome.tone}`}>
                    {outcome.label}
                  </span>
                  <span className="rec-run-time" title={formatAbsolute(run.startedAt)}>
                    {formatRelative(run.startedAt)}
                  </span>
                  {detail !== null && (
                    <span className={run.error ? "run-error" : "run-summary"}>
                      {detail}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        )}
      </details>
      {logRun && <LogViewer run={logRun} onClose={() => setLogRun(null)} />}
    </>
  );
}
