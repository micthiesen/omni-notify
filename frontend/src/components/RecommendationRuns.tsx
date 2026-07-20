import { useEffect, useState } from "react";
import { fetchTaskRuns } from "../api";
import type { TaskRun } from "../api";
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
  const [runs, setRuns] = useState<TaskRun[]>([]);
  const [logRun, setLogRun] = useState<TaskRun | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchTaskRuns({ task: taskName, limit: 6 })
      .then((data) => {
        if (!cancelled) setRuns(data.runs);
      })
      .catch(() => {
        // Recommendation cards remain useful if activity history is unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, [taskName, latestRunId]);

  return (
    <>
      <section className="page-section rec-activity-section">
        <h2 className="section-title">Recent Recommendation Runs</h2>
        {runs.length === 0 ? (
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
                  <span
                    className="rec-run-time"
                    title={formatAbsolute(run.startedAt)}
                  >
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
      </section>
      {logRun && <LogViewer run={logRun} onClose={() => setLogRun(null)} />}
    </>
  );
}
