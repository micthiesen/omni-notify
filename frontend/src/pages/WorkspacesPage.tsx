import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ApiError,
  fetchWorkspace,
  fetchRunLogs,
  fetchWorkspaces,
  fetchWorkspaceSubject,
  resolveWorkspaceAction,
  sendWorkspaceMessage,
  setWorkspaceSubjectStatus,
  type WorkspaceDetailResponse,
  type WorkspaceSubjectStatus,
  type WorkspaceSummary,
  type TaskRun,
} from "../api";
import { Link } from "../router";
import { formatAbsolute, formatRelative } from "../utils/format";

interface Props {
  workspaceId?: string;
  subjectId?: string;
}

export default function WorkspacesPage({ workspaceId, subjectId }: Props) {
  if (workspaceId && subjectId) {
    return <SubjectPage workspaceId={workspaceId} subjectId={subjectId} />;
  }
  return <WorkspaceList workspaceId={workspaceId} />;
}

function WorkspaceList({ workspaceId }: { workspaceId?: string }) {
  const [workspaces, setWorkspaces] = useState<WorkspaceSummary[] | null>(null);
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [busyWorkspaceId, setBusyWorkspaceId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    if (workspaceId) {
      const response = await fetchWorkspace(workspaceId);
      setWorkspaces([
        {
          ...response.workspace,
          subjects: response.subjects,
          activeSubjectCount: response.subjects.filter((s) => s.status === "active")
            .length,
          pendingActionCount: response.actions.filter((a) => a.status === "pending")
            .length,
          openPapercutCount: response.papercuts.length,
        },
      ]);
    } else {
      setWorkspaces((await fetchWorkspaces()).workspaces);
    }
  }, [workspaceId]);

  useEffect(() => {
    void load().catch((err) => setError(messageFor(err)));
  }, [load]);

  const send = async (id: string) => {
    const message = messages[id]?.trim();
    if (!message || busyWorkspaceId) return;
    setBusyWorkspaceId(id);
    setError(null);
    try {
      const { runId } = await sendWorkspaceMessage(id, message);
      await waitForRun(runId);
      setMessages((current) => ({ ...current, [id]: "" }));
      await load();
      window.dispatchEvent(new Event("workspace-updated"));
    } catch (err) {
      setError(messageFor(err));
    } finally {
      setBusyWorkspaceId(null);
    }
  };

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>
            {workspaceId ? (workspaces?.[0]?.title ?? "Workspace") : "Workspaces"}
          </h1>
          <p className="page-subtitle">
            Ongoing projects with durable context, practical artifacts, and
            human-approved actions.
          </p>
        </div>
      </div>
      {error && <div className="error">{error}</div>}
      {!workspaces && !error && <div className="loading">Loading…</div>}
      <div className="workspace-grid">
        {workspaces?.map((workspace) => (
          <section className="workspace-card" key={workspace.id}>
            <div className="workspace-card-header">
              <div>
                <h2>{workspace.title}</h2>
                <p>{workspace.description}</p>
              </div>
              <div className="workspace-counts meta-row">
                <span>{workspace.activeSubjectCount} Active</span>
                <span>{workspace.pendingActionCount} Pending</span>
                <span>{workspace.openPapercutCount} Papercuts</span>
                {workspace.scheduledRuns === false && <span>On Demand</span>}
              </div>
            </div>
            <div className="workspace-subject-list">
              {workspace.subjects.map((subject) => (
                <Link
                  key={subject.subjectId}
                  to={`/workspaces/${encodeURIComponent(workspace.id)}/${encodeURIComponent(subject.subjectId)}`}
                  className="workspace-subject-row"
                >
                  <span>
                    <strong>{subject.title}</strong>
                    <small>{subject.summary || "Work just started."}</small>
                  </span>
                  <span className={`workspace-status status-${subject.status}`}>
                    {subject.status}
                  </span>
                </Link>
              ))}
              {workspace.subjects.length === 0 && (
                <div className="muted">
                  Message the workspace to start the first{" "}
                  {workspace.subjectLabel.toLowerCase()}.
                </div>
              )}
            </div>
            <form
              className="workspace-compose"
              onSubmit={(event) => {
                event.preventDefault();
                void send(workspace.id);
              }}
            >
              <textarea
                value={messages[workspace.id] ?? ""}
                onChange={(event) =>
                  setMessages((current) => ({
                    ...current,
                    [workspace.id]: event.target.value,
                  }))
                }
                placeholder={
                  workspace.inputPlaceholder ??
                  `What would you like help with for this ${workspace.subjectLabel.toLowerCase()}?`
                }
                aria-label={`Message ${workspace.title}`}
              />
              <button
                type="submit"
                disabled={busyWorkspaceId !== null || !messages[workspace.id]?.trim()}
              >
                {busyWorkspaceId === workspace.id ? "Starting…" : "Send"}
              </button>
            </form>
            {!workspaceId && (
              <Link to={`/workspaces/${workspace.id}`} className="workspace-open-link">
                Open Workspace ›
              </Link>
            )}
          </section>
        ))}
      </div>
    </>
  );
}

function SubjectPage({
  workspaceId,
  subjectId,
}: {
  workspaceId: string;
  subjectId: string;
}) {
  const [detail, setDetail] = useState<WorkspaceDetailResponse | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    setDetail(await fetchWorkspaceSubject(workspaceId, subjectId));
  }, [workspaceId, subjectId]);

  useEffect(() => {
    void load().catch((err) => setError(messageFor(err)));
  }, [load]);

  useEffect(() => {
    if (!detail) return;
    const target = new URLSearchParams(window.location.search).get("target");
    if (!target) return;
    const element = document.getElementById(target);
    if (!element) return;
    element.classList.add("deep-link-target");
    element.scrollIntoView({ behavior: "smooth", block: "center" });
  }, [detail]);

  const revisions = useMemo(() => {
    const counts = new Map<string, number>();
    for (const item of detail?.artifactRevisions ?? []) {
      counts.set(item.artifactKey, (counts.get(item.artifactKey) ?? 0) + 1);
    }
    return counts;
  }, [detail]);

  const send = async () => {
    if (!message.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const { runId } = await sendWorkspaceMessage(
        workspaceId,
        message.trim(),
        subjectId,
      );
      await waitForRun(runId);
      setMessage("");
      await load();
      window.dispatchEvent(new Event("workspace-updated"));
    } catch (err) {
      setError(messageFor(err));
    } finally {
      setBusy(false);
    }
  };

  const resolveAction = async (actionId: string, resolution: "approve" | "reject") => {
    setBusy(true);
    try {
      await resolveWorkspaceAction(actionId, resolution);
      await load();
      window.dispatchEvent(new Event("workspace-updated"));
    } catch (err) {
      setError(messageFor(err));
    } finally {
      setBusy(false);
    }
  };

  const setStatus = async (status: WorkspaceSubjectStatus) => {
    setBusy(true);
    setError(null);
    try {
      await setWorkspaceSubjectStatus(workspaceId, subjectId, status);
      await load();
      window.dispatchEvent(new Event("workspace-updated"));
    } catch (err) {
      setError(messageFor(err));
    } finally {
      setBusy(false);
    }
  };

  if (!detail)
    return error ? (
      <div className="error">{error}</div>
    ) : (
      <div className="loading">Loading…</div>
    );
  return (
    <>
      <div className="page-header" id="workspace-summary">
        <div className="page-header-stack">
          <Link to={`/workspaces/${workspaceId}`} className="workspace-back">
            ← {detail.workspace.title}
          </Link>
          <h1>{detail.subject.title}</h1>
          <p className="page-subtitle">{detail.subject.summary}</p>
        </div>
        <select
          className="workspace-status-select"
          value={detail.subject.status}
          onChange={(event) =>
            void setStatus(event.target.value as WorkspaceSubjectStatus)
          }
          aria-label="Subject status"
        >
          <option value="active">Active</option>
          <option value="paused">Paused</option>
          <option value="completed">Completed</option>
          <option value="archived">Archived</option>
        </select>
      </div>
      {error && <div className="error">{error}</div>}

      {detail.actions.length > 0 && (
        <section className="workspace-section" id="actions">
          <h2>Actions</h2>
          <div className="workspace-action-list">
            {detail.actions.map((action) => (
              <article
                className="workspace-action-card"
                id={`action-${action.actionId}`}
                key={action.actionId}
              >
                <div className="workspace-action-heading">
                  <div>
                    <strong>{action.title}</strong>
                    <p>{action.description}</p>
                  </div>
                  <span className={`workspace-status status-${action.status}`}>
                    {action.status}
                  </span>
                </div>
                <pre>{formatActionPayload(action.payload)}</pre>
                {action.result && (
                  <p className="workspace-action-result">{action.result}</p>
                )}
                {(action.status === "pending" || action.status === "failed") && (
                  <div className="workspace-action-buttons">
                    <button
                      type="button"
                      onClick={() => void resolveAction(action.actionId, "approve")}
                      disabled={busy}
                    >
                      {action.status === "failed" ? "Retry" : "Approve"}
                    </button>
                    {action.status === "pending" && (
                      <button
                        type="button"
                        className="danger-button"
                        onClick={() => void resolveAction(action.actionId, "reject")}
                        disabled={busy}
                      >
                        Reject
                      </button>
                    )}
                  </div>
                )}
              </article>
            ))}
          </div>
        </section>
      )}

      <form
        className="workspace-compose workspace-compose-detail"
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <textarea
          value={message}
          onChange={(event) => setMessage(event.target.value)}
          placeholder={
            detail.workspace.followUpPlaceholder ??
            `Add details or ask for the next step on this ${detail.workspace.subjectLabel.toLowerCase()}…`
          }
          aria-label="Message workspace"
        />
        <button type="submit" disabled={busy || !message.trim()}>
          {busy ? "Working…" : "Send"}
        </button>
      </form>

      <section className="workspace-section" id="artifacts">
        <h2>Artifacts</h2>
        <div className="workspace-artifact-grid">
          {detail.workspace.artifacts.map((definition) => {
            const artifact = detail.artifacts.find(
              (item) => item.artifactKey === definition.key,
            );
            return (
              <article
                className="workspace-artifact-card"
                id={`artifact-${definition.key}`}
                key={definition.key}
              >
                <div className="workspace-artifact-heading">
                  <h3>{definition.title}</h3>
                  {artifact && (
                    <span>
                      {revisions.get(definition.key)} revision
                      {revisions.get(definition.key) === 1 ? "" : "s"}
                    </span>
                  )}
                </div>
                {artifact ? (
                  <>
                    <div className="workspace-artifact-content">{artifact.content}</div>
                    <small>Updated {formatRelative(artifact.createdAt)}</small>
                  </>
                ) : (
                  <p className="muted">Not created yet.</p>
                )}
              </article>
            );
          })}
        </div>
      </section>

      <div className="workspace-two-column">
        <section className="workspace-section" id="conversation">
          <h2>Conversation</h2>
          <div className="workspace-message-list">
            {detail.messages.map((item) => (
              <article
                className={`workspace-message workspace-message-${item.role}`}
                key={item.messageId}
              >
                <strong>{item.role === "user" ? "You" : "Omni"}</strong>
                <p>{item.text}</p>
                <small>{formatAbsolute(item.createdAt)}</small>
              </article>
            ))}
          </div>
        </section>
        <section className="workspace-section" id="sources">
          <h2>Sources</h2>
          {detail.emailScope && (
            <div className="workspace-scope">
              <strong>Email Scope</strong>
              <code>{JSON.stringify(detail.emailScope)}</code>
            </div>
          )}
          <div className="workspace-source-list">
            {detail.sources.map((source) => (
              <article key={source.sourceId}>
                <span className="workspace-source-kind">{source.kind}</span>
                {safeSourceHref(source.url) ? (
                  <a href={safeSourceHref(source.url)} target="_blank" rel="noreferrer">
                    {source.title}
                  </a>
                ) : (
                  <strong>{source.title}</strong>
                )}
                <p>{source.excerpt}</p>
              </article>
            ))}
            {detail.sources.length === 0 && (
              <p className="muted">No sources captured yet.</p>
            )}
          </div>
        </section>
      </div>
      {detail.papercuts.length > 0 && (
        <section className="workspace-section workspace-papercuts">
          <h2>Papercuts</h2>
          {detail.papercuts.map((item) => (
            <article key={item.papercutId}>
              <strong>{item.title}</strong>
              <p>{item.detail}</p>
              <small>
                {item.occurrences} occurrence{item.occurrences === 1 ? "" : "s"}
              </small>
            </article>
          ))}
        </section>
      )}
    </>
  );
}

function messageFor(error: unknown): string {
  return error instanceof Error ? error.message : "Something went wrong";
}

function formatActionPayload(payload: string): string {
  try {
    return JSON.stringify(JSON.parse(payload), null, 2);
  } catch {
    return "Stored action details are unavailable.";
  }
}

function safeSourceHref(value: string | undefined): string | undefined {
  if (!value) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.toString()
      : undefined;
  } catch {
    return undefined;
  }
}

async function waitForRun(runId: string): Promise<void> {
  const deadline = Date.now() + 5 * 60_000;
  while (Date.now() < deadline) {
    let run: TaskRun;
    try {
      ({ run } = await fetchRunLogs(runId));
    } catch (error) {
      if (!(error instanceof ApiError) || error.status !== 404) throw error;
      await new Promise((resolve) => window.setTimeout(resolve, 250));
      continue;
    }
    if (run.status === "success") return;
    if (run.status === "error") throw new Error(run.error ?? "Workspace run failed");
    await new Promise((resolve) => window.setTimeout(resolve, 1_000));
  }
  throw new Error(
    "Workspace research is still running. Refresh shortly to see the result.",
  );
}
