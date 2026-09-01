import { useEffect, useMemo, useState } from "react";
import { fetchEmailActivity, fetchWorkspaces, type WorkspaceSummary } from "../api";
import { LiveNow } from "../components/LiveNow";
import { OnDeck } from "../components/OnDeck";
import { StatStrip } from "../components/StatStrip";
import { useLiveData } from "../live";
import { Link } from "../router";
import { formatRelative } from "../utils/format";

interface HomeResearchData {
  workspaces: WorkspaceSummary[] | null;
  recentEmailProblems: number | null;
  workspaceError: boolean;
  emailError: boolean;
}

export default function HomePage() {
  const { snapshot, error } = useLiveData();
  const [research, setResearch] = useState<HomeResearchData>({
    workspaces: null,
    recentEmailProblems: null,
    workspaceError: false,
    emailError: false,
  });
  const latestRunAt = snapshot?.runs[0]?.finishedAt ?? 0;

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      fetchWorkspaces()
        .then((response) => {
          if (!cancelled) {
            setResearch((current) => ({
              ...current,
              workspaces: response.workspaces,
              workspaceError: false,
            }));
          }
        })
        .catch(() => {
          if (!cancelled) {
            setResearch((current) => ({ ...current, workspaceError: true }));
          }
        });
      fetchEmailActivity(undefined, 500)
        .then((response) => {
          if (cancelled) return;
          const since = Date.now() - 24 * 60 * 60 * 1000;
          setResearch((current) => ({
            ...current,
            recentEmailProblems: response.activities.filter(
              (activity) =>
                activity.processedAt >= since &&
                ["partial", "failed", "error"].includes(activity.outcome),
            ).length,
            emailError: false,
          }));
        })
        .catch(() => {
          if (!cancelled) {
            setResearch((current) => ({ ...current, emailError: true }));
          }
        });
    };
    refresh();
    window.addEventListener("workspace-updated", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("workspace-updated", refresh);
    };
  }, [latestRunAt]);

  const activeSubjects = useMemo(
    () =>
      (research.workspaces ?? [])
        .flatMap((workspace) =>
          workspace.subjects
            .filter((subject) => subject.status === "active")
            .map((subject) => ({ workspace, subject })),
        )
        .sort((a, b) => b.subject.updatedAt - a.subject.updatedAt)
        .slice(0, 4),
    [research.workspaces],
  );

  if (snapshot === null) {
    return error !== null ? (
      <div className="error">
        <div>Failed to load home</div>
        <div className="error-detail">{error}</div>
      </div>
    ) : (
      <div className="loading">Loading…</div>
    );
  }

  const failingTasks = snapshot.tasks.filter(
    (task) => task.lastRun?.status === "error",
  );
  const pendingActions = (research.workspaces ?? []).reduce(
    (total, workspace) => total + workspace.pendingActionCount,
    0,
  );
  const openPapercuts = (research.workspaces ?? []).reduce(
    (total, workspace) => total + workspace.openPapercutCount,
    0,
  );
  const attentionCount =
    failingTasks.length +
    pendingActions +
    openPapercuts +
    (research.recentEmailProblems ?? 0);
  const attentionUnavailable = research.workspaceError || research.emailError;

  return (
    <>
      {error && (
        <div className="error-inline stale-note">
          Refresh failed ({error}), showing last known state.
        </div>
      )}

      <section className="attention-panel">
        <div className="attention-heading">
          <div>
            <span className="section-title">Needs Attention</span>
            <p>
              {attentionUnavailable
                ? `${attentionCount} known item${attentionCount === 1 ? "" : "s"}; some status is unavailable.`
                : attentionCount === 0
                  ? "Everything is running cleanly."
                  : `${attentionCount} item${attentionCount === 1 ? "" : "s"} need a look.`}
            </p>
          </div>
          <span
            className={`attention-total ${attentionCount > 0 || attentionUnavailable ? "has-items" : ""}`}
          >
            {attentionUnavailable ? "!" : attentionCount}
          </span>
        </div>
        {attentionCount > 0 && (
          <div className="attention-links">
            {pendingActions > 0 && (
              <Link to="/workspaces" className="attention-item">
                <strong>{pendingActions}</strong>
                <span>Research Approval{pendingActions === 1 ? "" : "s"}</span>
              </Link>
            )}
            {failingTasks.length > 0 && (
              <Link to="/operations" className="attention-item attention-danger">
                <strong>{failingTasks.length}</strong>
                <span>Failed Task{failingTasks.length === 1 ? "" : "s"}</span>
              </Link>
            )}
            {(research.recentEmailProblems ?? 0) > 0 && (
              <Link to="/emails" className="attention-item attention-danger">
                <strong>{research.recentEmailProblems}</strong>
                <span>Email Issue{research.recentEmailProblems === 1 ? "" : "s"}</span>
              </Link>
            )}
            {openPapercuts > 0 && (
              <Link to="/workspaces" className="attention-item">
                <strong>{openPapercuts}</strong>
                <span>Papercut{openPapercuts === 1 ? "" : "s"}</span>
              </Link>
            )}
          </div>
        )}
        {attentionUnavailable && (
          <div className="attention-unavailable">
            {research.workspaceError && (
              <span>Research status could not be refreshed.</span>
            )}
            {research.emailError && <span>Email status could not be refreshed.</span>}
          </div>
        )}
      </section>

      <LiveNow streamers={snapshot.streamers} />
      <OnDeck items={snapshot.onDeck} />

      <section className="page-section home-research">
        <div className="section-heading-row">
          <h2 className="section-title">Research</h2>
          <Link to="/workspaces" className="section-view-all">
            View Workspaces ›
          </Link>
        </div>
        {activeSubjects.length > 0 ? (
          <div className="home-research-grid">
            {activeSubjects.map(({ workspace, subject }) => (
              <Link
                key={`${workspace.id}/${subject.subjectId}`}
                to={`/workspaces/${encodeURIComponent(workspace.id)}/${encodeURIComponent(subject.subjectId)}`}
                className="home-research-card"
              >
                <span className="home-research-workspace">{workspace.title}</span>
                <strong>{subject.title}</strong>
                <p>{subject.summary || "Research in progress."}</p>
                <small>Updated {formatRelative(subject.updatedAt)}</small>
              </Link>
            ))}
          </div>
        ) : (
          <Link to="/workspaces" className="home-research-empty">
            Start an ongoing workspace ›
          </Link>
        )}
      </section>

      <section className="page-section home-system-health">
        <div className="section-heading-row">
          <h2 className="section-title">System Health</h2>
          <Link to="/operations" className="section-view-all">
            Open Operations ›
          </Link>
        </div>
        <StatStrip snapshot={snapshot} />
      </section>
    </>
  );
}
