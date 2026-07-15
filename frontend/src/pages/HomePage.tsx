import { ActivityFeed } from "../components/ActivityFeed";
import { LiveNow } from "../components/LiveNow";
import { StatStrip } from "../components/StatStrip";
import { TaskCard } from "../components/TaskCard";
import { Toast, useToast } from "../components/Toast";
import { useLiveData } from "../live";

export default function HomePage() {
  const { snapshot, error, runTask } = useLiveData();
  const { toast, showToast } = useToast();

  const handleRun = async (name: string) => {
    const result = await runTask(name);
    showToast(result.message, result.ok ? "info" : "error");
  };

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
            {snapshot.tasks.map((task) => (
              <TaskCard key={task.name} task={task} onRun={handleRun} />
            ))}
          </div>
        )}
      </section>

      <section className="page-section">
        <h2 className="section-title">Activity</h2>
        <ActivityFeed snapshot={snapshot} />
      </section>
    </>
  );
}
