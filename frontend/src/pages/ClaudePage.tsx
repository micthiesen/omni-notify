import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  type ApiClientError,
  type ClaudeActivityResponse,
  type ClaudeLinkError,
  type ClaudeLinkStatus,
  type ClaudeSession,
  type ClaudeTranscript,
  type ClaudeTranscriptItem,
  type McpCall,
  fetchClaudeActivity,
  fetchClaudeSessions,
  fetchClaudeTranscript,
} from "../api";
import { CallStatusPill } from "../components/McpBadges";
import { WorkspaceMarkdown } from "../components/WorkspaceMarkdown";
import { forkUiRequest } from "../effect";
import { useModal } from "../hooks/useModal";
import { useNow } from "../hooks/useNow";
import { useVisiblePoll } from "../hooks/useVisiblePoll";
import { Link } from "../router";
import {
  ACTION_LABELS,
  type ClaudeActionGroup,
  booleanField,
  claudeActionKind,
  explainClaudeError,
  groupClaudeActions,
  numberField,
  outputSession,
  parseIsoMs,
  shortSessionId,
  stringField,
  summarizeCompactAction,
} from "../utils/claudeActivity";
import { formatAbsolute, formatDuration, formatRelative } from "../utils/format";

const TIMELINE_PAGE = 6;

function errorText(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

function isLinkError(
  error: ApiClientError | ClaudeLinkError,
): error is ClaudeLinkError {
  return error._tag === "ClaudeLinkError";
}

// ===== Link status =====

type LinkState = "online" | "offline" | "disabled" | "unconfigured";

function linkState(link: ClaudeLinkStatus): LinkState {
  if (!link.configured) return "unconfigured";
  if (link.disabled) return "disabled";
  return link.online ? "online" : "offline";
}

const LINK_COPY: Record<LinkState, { label: string; detail: string }> = {
  online: {
    label: "Online",
    detail: "The Mac is polling Omni for Claude Code jobs.",
  },
  offline: {
    label: "Offline",
    detail:
      "The Mac has stopped polling. It may be asleep, away from home without VPN, or the omni-link agent stopped. Jobs sent now will not be picked up.",
  },
  disabled: {
    label: "Disabled",
    detail:
      "The kill switch is on, so Claude tools refuse to act. Run `omni-link enable` on the Mac to turn it back on.",
  },
  unconfigured: {
    label: "Not Configured",
    detail: "The Mac device link is not configured on this server.",
  },
};

function LinkHero({ link, now }: { link: ClaudeLinkStatus; now: number }) {
  const state = linkState(link);
  const lastSeen = parseIsoMs(link.lastSeenAt);
  return (
    <section
      className={`claude-hero claude-hero-${state}`}
      aria-label="Mac link status"
    >
      <div className="claude-hero-status">
        <span className="claude-hero-dot" aria-hidden="true" />
        <div className="claude-hero-text">
          <div className="claude-hero-label">
            Mac Link · <strong>{LINK_COPY[state].label}</strong>
          </div>
          <p className="claude-hero-detail">{LINK_COPY[state].detail}</p>
        </div>
      </div>
      <dl className="claude-hero-facts">
        <div>
          <dt>Host</dt>
          <dd className="claude-mono">{link.host ?? "—"}</dd>
        </div>
        <div>
          <dt>Last Seen</dt>
          <dd title={lastSeen === null ? undefined : formatAbsolute(lastSeen)}>
            {lastSeen === null ? "Never" : formatRelative(lastSeen, now)}
          </dd>
        </div>
        <div>
          <dt>Pending Jobs</dt>
          <dd className={link.pendingJobs > 0 ? "claude-hero-pending" : undefined}>
            {link.pendingJobs}
          </dd>
        </div>
      </dl>
    </section>
  );
}

function LinkErrorNotice({ error }: { error: ClaudeLinkError }) {
  const { hint } = explainClaudeError(error.code);
  return (
    <div className={`claude-link-notice claude-link-notice-${error.code}`}>
      <strong>{error.message}</strong>
      {hint && <span>{hint}</span>}
    </div>
  );
}

// ===== Shared bits =====

function SessionStatus({ status }: { status: string | null }) {
  if (!status) return null;
  return (
    <span className={`claude-status claude-status-${status}`}>
      <span className="claude-status-dot" aria-hidden="true" />
      {status}
    </span>
  );
}

function ExpandableText({
  text,
  className,
  lines = 6,
}: {
  text: string;
  className: string;
  lines?: number;
}) {
  const [open, setOpen] = useState(false);
  const long = text.length > lines * 90 || text.split("\n").length > lines;
  return (
    <div className="claude-expandable">
      <div
        className={`${className} ${long && !open ? "claude-clamped" : ""}`}
        style={long && !open ? { WebkitLineClamp: lines } : undefined}
      >
        {text}
      </div>
      {long && (
        <button
          type="button"
          className="claude-more-btn"
          onClick={() => setOpen((value) => !value)}
        >
          {open ? "Show less" : "Show more"}
        </button>
      )}
    </div>
  );
}

// ===== Transcript =====

function TranscriptItemView({ item }: { item: ClaudeTranscriptItem }) {
  const time = parseIsoMs(item.timestamp);
  const stamp =
    time === null ? null : (
      <time className="claude-tx-time" title={formatAbsolute(time)}>
        {new Date(time).toLocaleTimeString("en-US", {
          hour: "numeric",
          minute: "2-digit",
        })}
      </time>
    );
  const truncated = item.truncated && <span className="claude-tag">truncated</span>;
  switch (item.kind) {
    case "user":
      return (
        <div className="claude-tx-row claude-tx-user">
          <div className="claude-bubble claude-bubble-user">
            {item.text ?? ""}
            <div className="claude-tx-foot">
              {truncated}
              {stamp}
            </div>
          </div>
        </div>
      );
    case "assistant":
      return (
        <div className="claude-tx-row claude-tx-assistant">
          <div className="claude-bubble claude-bubble-assistant">
            <WorkspaceMarkdown content={item.text ?? ""} />
            <div className="claude-tx-foot">
              {truncated}
              {stamp}
            </div>
          </div>
        </div>
      );
    case "tool_use":
      return (
        <div className="claude-tx-row">
          <details className="claude-tool-chip">
            <summary>
              <span className="claude-tool-name">{item.tool ?? "tool"}</span>
              {item.input && <span className="claude-tool-input">{item.input}</span>}
            </summary>
            {item.input && <pre className="mcp-json">{item.input}</pre>}
          </details>
        </div>
      );
    case "tool_result":
      return (
        <div className="claude-tx-row">
          <details
            className={`claude-tool-result ${item.isError ? "claude-tool-result-error" : ""}`}
          >
            <summary>
              {item.isError ? "Tool error" : "Tool result"}
              {item.tool && <span className="claude-tool-name">{item.tool}</span>}
              {truncated}
            </summary>
            <pre className="mcp-json">{item.text ?? "(empty)"}</pre>
          </details>
        </div>
      );
    default:
      return (
        <div className="claude-tx-row">
          <div className="claude-tx-other muted">
            <span className="claude-tag">{item.kind}</span> {item.text ?? ""}
          </div>
        </div>
      );
  }
}

function TranscriptModal({
  sessionId,
  title,
  onClose,
}: {
  sessionId: string;
  title: string;
  onClose: () => void;
}) {
  const [transcript, setTranscript] = useState<ClaudeTranscript | null>(null);
  const [error, setError] = useState<ApiClientError | ClaudeLinkError | null>(null);
  const [loading, setLoading] = useState(true);
  const [version, setVersion] = useState(0);
  const modalRef = useModal(onClose);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setLoading(true);
    return forkUiRequest(fetchClaudeTranscript(sessionId, { limit: 40 }), {
      onSuccess: (next) => {
        setTranscript(next);
        setError(null);
        setLoading(false);
      },
      onFailure: (err) => {
        setError(err);
        setLoading(false);
      },
    });
  }, [sessionId, version]);

  useEffect(() => {
    const body = bodyRef.current;
    if (body && transcript) body.scrollTop = body.scrollHeight;
  }, [transcript]);

  return (
    <div className="modal-root">
      <button
        type="button"
        className="modal-backdrop"
        tabIndex={-1}
        onClick={onClose}
        aria-label="Close transcript"
      />
      <div
        className="log-modal claude-tx-modal"
        ref={modalRef}
        tabIndex={-1}
        aria-modal="true"
        role="dialog"
        aria-label={`Transcript for ${title}`}
      >
        <div className="log-modal-header">
          <div className="log-modal-title">
            <span className="log-modal-task">{title}</span>
          </div>
          <div className="log-modal-meta meta-row muted">
            <span className="claude-mono">{shortSessionId(sessionId)}</span>
            {transcript && <span>rev {transcript.revision}</span>}
            <button
              type="button"
              className="claude-link-btn"
              onClick={() => setVersion((v) => v + 1)}
              disabled={loading}
            >
              {loading ? "Loading…" : "Refresh"}
            </button>
          </div>
          <button
            type="button"
            className="log-modal-close"
            onClick={onClose}
            aria-label="Close"
          >
            ✕
          </button>
        </div>
        <div className="claude-tx-body" ref={bodyRef}>
          {transcript?.hasMore && (
            <div className="claude-tx-note muted">
              Showing the latest {transcript.items.length} items. Earlier history stays
              on the Mac.
            </div>
          )}
          {error &&
            (isLinkError(error) ? (
              <LinkErrorNotice error={error} />
            ) : (
              <div className="error-inline">
                {errorText(error, "Failed to load transcript")}
              </div>
            ))}
          {!transcript && !error && <div className="loading-inline">Loading…</div>}
          {transcript && transcript.items.length === 0 && (
            <div className="muted claude-tx-note">No transcript items yet.</div>
          )}
          {transcript?.items.map((item) => (
            <TranscriptItemView key={item.index} item={item} />
          ))}
        </div>
      </div>
    </div>
  );
}

// ===== Live sessions =====

function SessionCard({
  session,
  now,
  onOpen,
}: {
  session: ClaudeSession;
  now: number;
  onOpen: () => void;
}) {
  const started = parseIsoMs(session.startedAt);
  return (
    <article className={`claude-session-card claude-session-${session.status}`}>
      <header className="claude-session-head">
        <h3 className="claude-session-title">{session.title || "Untitled session"}</h3>
        <SessionStatus status={session.status} />
      </header>
      <div className="claude-session-meta meta-row muted">
        {session.project && <span className="claude-project">{session.project}</span>}
        {session.kind && <span>{session.kind}</span>}
        {started !== null && (
          <span title={formatAbsolute(started)}>
            started {formatRelative(started, now)}
          </span>
        )}
        <span>rev {session.revision}</span>
        <span className="claude-mono">
          {session.id ?? shortSessionId(session.sessionId)}
        </span>
      </div>
      {session.lastAssistant ? (
        <p className="claude-session-excerpt">{session.lastAssistant}</p>
      ) : (
        <p className="claude-session-excerpt muted">No assistant reply yet.</p>
      )}
      <div className="claude-session-actions">
        <button type="button" className="run-btn" onClick={onOpen}>
          Transcript
        </button>
      </div>
    </article>
  );
}

function SessionsSection({
  now,
  onOpenTranscript,
}: {
  now: number;
  onOpenTranscript: (sessionId: string, title: string) => void;
}) {
  const [includeStopped, setIncludeStopped] = useState(false);
  const [sessions, setSessions] = useState<ClaudeSession[] | null>(null);
  const [error, setError] = useState<ApiClientError | ClaudeLinkError | null>(null);
  const [project, setProject] = useState("");

  useVisiblePoll(
    `sessions:${includeStopped}`,
    () => fetchClaudeSessions({ includeStopped, limit: 25 }),
    {
      onSuccess: (response) => {
        setSessions(response.sessions);
        setError(null);
      },
      onFailure: (err) => {
        setError(err);
        // The Mac is unreachable; a stale list would suggest it is still live.
        if (isLinkError(err)) setSessions(null);
      },
    },
  );

  const projects = useMemo(() => {
    const counts = new Map<string, number>();
    for (const session of sessions ?? []) {
      const key = session.project ?? "Other";
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [sessions]);

  const visible = (sessions ?? []).filter(
    (session) => project === "" || (session.project ?? "Other") === project,
  );
  const busy = (sessions ?? []).filter((session) => session.status === "busy").length;

  return (
    <section className="page-section">
      <div className="section-heading-row">
        <h2 className="section-title">
          On the Mac
          {sessions && <span className="section-count">{sessions.length}</span>}
          {busy > 0 && <span className="claude-busy-count">{busy} busy</span>}
        </h2>
      </div>
      <div className="task-toolbar">
        <div className="task-filters" role="group" aria-label="Filter by project">
          <button
            type="button"
            className={`chip-btn ${project === "" ? "active" : ""}`}
            aria-pressed={project === ""}
            onClick={() => setProject("")}
          >
            All
          </button>
          {projects.map(([name, count]) => (
            <button
              key={name}
              type="button"
              className={`chip-btn ${project === name ? "active" : ""}`}
              aria-pressed={project === name}
              onClick={() => setProject(project === name ? "" : name)}
            >
              {name} <span className="chip-btn-count">{count}</span>
            </button>
          ))}
        </div>
        <button
          type="button"
          className={`chip-btn ${includeStopped ? "active" : ""}`}
          aria-pressed={includeStopped}
          onClick={() => setIncludeStopped((value) => !value)}
        >
          Show stopped
        </button>
      </div>
      {error &&
        (isLinkError(error) ? (
          <LinkErrorNotice error={error} />
        ) : (
          <div className="error-inline">
            {sessions ? "Refresh failed: " : ""}
            {errorText(error, "Failed to load sessions")}
          </div>
        ))}
      {sessions === null && error === null && (
        <div className="loading-inline">Asking the Mac…</div>
      )}
      {sessions !== null && visible.length === 0 && (
        <div className="muted">
          {includeStopped ? "No sessions." : "No running sessions."}
        </div>
      )}
      {visible.length > 0 && (
        <div className="claude-session-grid">
          {visible.map((session) => (
            <SessionCard
              key={session.sessionId}
              session={session}
              now={now}
              onOpen={() =>
                onOpenTranscript(session.sessionId, session.title || "Untitled session")
              }
            />
          ))}
        </div>
      )}
    </section>
  );
}

// ===== Action timeline =====

function ErrorCallout({ error }: { error: string }) {
  const { code, hint } = explainClaudeError(error);
  return (
    <div
      className={`claude-error ${code === "outcome_unknown" ? "claude-error-warn" : ""}`}
    >
      {code && <span className="claude-error-code">{code}</span>}
      <span className="claude-error-message">{error}</span>
      {hint && <span className="claude-error-hint">{hint}</span>}
    </div>
  );
}

function Tag({ children, tone }: { children: ReactNode; tone?: string }) {
  return (
    <span className={`claude-tag ${tone ? `claude-tag-${tone}` : ""}`}>{children}</span>
  );
}

function ActionBody({ call }: { call: McpCall }) {
  const kind = claudeActionKind(call.tool);
  const input = call.input;
  const output = call.output;
  const session = outputSession(call);
  const prompt = stringField(input, "prompt");

  switch (kind) {
    case "start": {
      const model = stringField(input, "model");
      const effort = stringField(input, "effort");
      const title = stringField(input, "title");
      const project = stringField(input, "project");
      return (
        <>
          <div className="claude-action-tags">
            {project && <span className="claude-project">{project}</span>}
            {model && <Tag>{model}</Tag>}
            {effort && <Tag>effort {effort}</Tag>}
            {booleanField(output, "reused") && <Tag tone="accent">reused</Tag>}
          </div>
          {title && <div className="claude-action-line">{title}</div>}
          {prompt && <ExpandableText text={prompt} className="claude-quote" />}
        </>
      );
    }
    case "send": {
      const warning = stringField(output, "warning");
      return (
        <>
          {booleanField(input, "interrupt") && (
            <div className="claude-action-tags">
              <Tag tone="warn">interrupt</Tag>
            </div>
          )}
          {prompt && <ExpandableText text={prompt} className="claude-quote" />}
          {warning && <div className="claude-warning">{warning}</div>}
        </>
      );
    }
    case "wait": {
      const timedOut = booleanField(output, "timedOut");
      const timeout = numberField(input, "timeoutSeconds");
      if (timedOut === null) return null;
      return (
        <div className="claude-action-line">
          {timedOut ? (
            <>
              <Tag tone="warn">timed out</Tag>
              {timeout !== null && ` after ${timeout}s`}
              {session?.status && `, still ${session.status}`}
            </>
          ) : (
            <>
              <Tag tone="success">settled</Tag>
              {session?.status && ` ${session.status}`}
              {session?.revision !== null &&
                session?.revision !== undefined &&
                ` at rev ${session.revision}`}
            </>
          )}
        </div>
      );
    }
    case "result": {
      const result = stringField(output, "result");
      if (output === null) return null;
      return result ? (
        <div className="claude-result">
          <WorkspaceMarkdown content={result} />
          {booleanField(output, "truncated") && <Tag>truncated</Tag>}
        </div>
      ) : (
        <div className="claude-action-line muted">No result yet.</div>
      );
    }
    case "stop":
      return null;
    default:
      return (
        <div className="claude-action-line muted">{summarizeCompactAction(call)}</div>
      );
  }
}

function TimelineItem({ call, now }: { call: McpCall; now: number }) {
  const kind = claudeActionKind(call.tool);
  return (
    <li className={`claude-tl-item claude-tl-${kind} claude-tl-status-${call.status}`}>
      <span className="claude-tl-marker" aria-hidden="true" />
      <div className="claude-tl-content">
        <div className="claude-tl-head">
          <span className="claude-tl-label">{ACTION_LABELS[kind]}</span>
          {call.status !== "ok" && <CallStatusPill status={call.status} />}
          <span className="claude-tl-time" title={formatAbsolute(call.startedAt)}>
            {formatRelative(call.startedAt, now)}
            {call.durationMs !== null && ` · ${formatDuration(call.durationMs)}`}
          </span>
        </div>
        {call.error && <ErrorCallout error={call.error} />}
        <ActionBody call={call} />
      </div>
    </li>
  );
}

function ActionGroupCard({
  group,
  now,
  onOpenTranscript,
}: {
  group: ClaudeActionGroup;
  now: number;
  onOpenTranscript: (sessionId: string, title: string) => void;
}) {
  const [showAll, setShowAll] = useState(false);
  const hidden = showAll ? 0 : Math.max(0, group.actions.length - TIMELINE_PAGE);
  const shown = group.actions.slice(hidden);
  const title = group.title ?? (group.sessionId ? "Untitled session" : "Failed start");
  return (
    <article
      className={`claude-group ${group.latestFailed ? "claude-group-failed" : ""}`}
    >
      <header className="claude-group-head">
        <div className="claude-group-title-row">
          <h3 className="claude-group-title">{title}</h3>
          <SessionStatus status={group.status} />
        </div>
        <div className="claude-group-meta meta-row muted">
          {group.project && <span className="claude-project">{group.project}</span>}
          {group.sessionId && (
            <span className="claude-mono" title={group.sessionId}>
              {shortSessionId(group.sessionId)}
            </span>
          )}
          <span>
            {group.actions.length} action{group.actions.length === 1 ? "" : "s"}
          </span>
          <span title={formatAbsolute(group.latestAt)}>
            {formatRelative(group.latestAt, now)}
          </span>
          {group.sessionId && (
            <button
              type="button"
              className="claude-link-btn"
              onClick={() => onOpenTranscript(group.sessionId as string, title)}
            >
              Transcript
            </button>
          )}
        </div>
      </header>
      {hidden > 0 && (
        <button
          type="button"
          className="claude-more-btn"
          onClick={() => setShowAll(true)}
        >
          Show {hidden} earlier action{hidden === 1 ? "" : "s"}
        </button>
      )}
      <ol className="claude-timeline">
        {shown.map((call) => (
          <TimelineItem key={call.callId} call={call} now={now} />
        ))}
      </ol>
    </article>
  );
}

function OtherActions({ calls, now }: { calls: McpCall[]; now: number }) {
  const [showAll, setShowAll] = useState(false);
  const shown = showAll ? calls : calls.slice(0, 8);
  return (
    <article className="claude-group claude-group-other">
      <header className="claude-group-head">
        <div className="claude-group-title-row">
          <h3 className="claude-group-title">Other</h3>
          <span className="muted claude-group-sub">Listings and status checks</span>
        </div>
      </header>
      <ul className="claude-other-list">
        {shown.map((call) => {
          const kind = claudeActionKind(call.tool);
          return (
            <li key={call.callId} className="claude-other-row">
              <span className="claude-other-label">{ACTION_LABELS[kind]}</span>
              <span className="claude-other-summary">
                {call.error ? (
                  <span className="claude-other-error">{call.error}</span>
                ) : (
                  summarizeCompactAction(call)
                )}
              </span>
              {call.status !== "ok" && <CallStatusPill status={call.status} />}
              <span className="claude-tl-time" title={formatAbsolute(call.startedAt)}>
                {formatRelative(call.startedAt, now)}
              </span>
            </li>
          );
        })}
      </ul>
      {!showAll && calls.length > shown.length && (
        <button
          type="button"
          className="claude-more-btn"
          onClick={() => setShowAll(true)}
        >
          Show {calls.length - shown.length} more
        </button>
      )}
    </article>
  );
}

function ActionsSection({
  actions,
  now,
  onOpenTranscript,
}: {
  actions: McpCall[];
  now: number;
  onOpenTranscript: (sessionId: string, title: string) => void;
}) {
  const [project, setProject] = useState("");
  const { sessions, other } = useMemo(() => groupClaudeActions(actions), [actions]);
  const projects = useMemo(() => {
    const counts = new Map<string, number>();
    for (const group of sessions) {
      if (group.project)
        counts.set(group.project, (counts.get(group.project) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [sessions]);
  const visible =
    project === "" ? sessions : sessions.filter((group) => group.project === project);

  return (
    <section className="page-section">
      <div className="section-heading-row">
        <h2 className="section-title">
          Actions <span className="section-count">{actions.length}</span>
        </h2>
        <Link to="/mcp" className="section-view-all">
          All MCP calls ›
        </Link>
      </div>
      {projects.length > 0 && (
        <div
          className="task-filters claude-filter-row"
          role="group"
          aria-label="Filter by project"
        >
          <button
            type="button"
            className={`chip-btn ${project === "" ? "active" : ""}`}
            aria-pressed={project === ""}
            onClick={() => setProject("")}
          >
            All
          </button>
          {projects.map(([name, count]) => (
            <button
              key={name}
              type="button"
              className={`chip-btn ${project === name ? "active" : ""}`}
              aria-pressed={project === name}
              onClick={() => setProject(project === name ? "" : name)}
            >
              {name} <span className="chip-btn-count">{count}</span>
            </button>
          ))}
        </div>
      )}
      {actions.length === 0 ? (
        <div className="muted">No Claude Code actions recorded yet.</div>
      ) : (
        <div className="claude-groups">
          {visible.map((group) => (
            <ActionGroupCard
              key={group.key}
              group={group}
              now={now}
              onOpenTranscript={onOpenTranscript}
            />
          ))}
          {project === "" && other.length > 0 && (
            <OtherActions calls={other} now={now} />
          )}
        </div>
      )}
    </section>
  );
}

// ===== Page =====

export default function ClaudePage() {
  const now = useNow(15_000);
  const [activity, setActivity] = useState<ClaudeActivityResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<{
    sessionId: string;
    title: string;
  } | null>(null);

  useVisiblePoll("activity", () => fetchClaudeActivity(200), {
    onSuccess: (response) => {
      setActivity(response);
      setError(null);
    },
    onFailure: (err) => setError(errorText(err, "Failed to load Claude activity")),
  });

  const openTranscript = (sessionId: string, title: string) =>
    setTranscript({ sessionId, title });

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>Claude Code</h1>
          <p className="page-subtitle">
            Sessions on the MacBook and what agents asked them to do.
          </p>
        </div>
      </div>

      {activity === null && error === null && <div className="loading">Loading…</div>}
      {activity === null && error !== null && (
        <div className="error">
          <div>Failed to load Claude activity</div>
          <div className="error-detail">{error}</div>
        </div>
      )}
      {activity !== null && (
        <>
          {error && (
            <div className="error-inline stale-note">
              Refresh failed ({error}), showing last known state.
            </div>
          )}
          <LinkHero link={activity.link} now={now} />
        </>
      )}

      <SessionsSection now={now} onOpenTranscript={openTranscript} />

      {activity !== null && (
        <ActionsSection
          actions={activity.actions}
          now={now}
          onOpenTranscript={openTranscript}
        />
      )}

      {transcript && (
        <TranscriptModal
          key={transcript.sessionId}
          sessionId={transcript.sessionId}
          title={transcript.title}
          onClose={() => setTranscript(null)}
        />
      )}
    </>
  );
}
