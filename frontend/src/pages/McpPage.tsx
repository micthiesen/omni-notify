import { useEffect, useMemo, useRef, useState } from "react";
import {
  type McpActivityResponse,
  type McpCall,
  type McpToolSummary,
  fetchMcpActivity,
} from "../api";
import { forkUiRequest } from "../effect";
import { CallStatusPill, PolicyBadge } from "../components/McpBadges";
import { useNow } from "../hooks/useNow";
import { useVisiblePoll } from "../hooks/useVisiblePoll";
import { Link } from "../router";
import { formatJson, isClaudeTool } from "../utils/claudeActivity";
import { formatAbsolute, formatDuration, formatRelative } from "../utils/format";

type StatusFilter = "all" | "error" | "running";

const STATUS_FILTERS: ReadonlyArray<[StatusFilter, string]> = [
  ["all", "All"],
  ["error", "Errors"],
  ["running", "Running"],
];

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

function StatTiles({ summary }: { summary: McpActivityResponse["summary"] }) {
  const tiles: Array<{
    label: string;
    value: number;
    tone?: "accent" | "danger" | "warn";
  }> = [
    { label: "Stored Calls", value: summary.stored },
    { label: "Last 24h", value: summary.last24h },
    {
      label: "Errors 24h",
      value: summary.errors24h,
      tone: summary.errors24h > 0 ? "danger" : undefined,
    },
    {
      label: "Running",
      value: summary.running,
      tone: summary.running > 0 ? "accent" : undefined,
    },
    {
      label: "Approval Calls 24h",
      value: summary.approvalCalls24h,
      tone: summary.approvalCalls24h > 0 ? "warn" : undefined,
    },
  ];
  return (
    <div className="stat-strip">
      {tiles.map((tile) => (
        <div key={tile.label} className={`stat-tile ${tile.tone ?? ""}`}>
          <span className="stat-label">{tile.label}</span>
          <span className="stat-value">{tile.value.toLocaleString()}</span>
        </div>
      ))}
    </div>
  );
}

function ToolSummary({
  tools,
  activeTool,
  onSelect,
  now,
}: {
  tools: McpToolSummary[];
  activeTool: string;
  onSelect: (tool: string) => void;
  now: number;
}) {
  if (tools.length === 0) return <div className="muted">No tools called yet.</div>;
  return (
    <div className="mcp-tool-grid">
      {tools.map((tool) => {
        const errorRate = tool.calls > 0 ? tool.errors / tool.calls : 0;
        return (
          <button
            key={tool.tool}
            type="button"
            className={`mcp-tool-card ${activeTool === tool.tool ? "active" : ""}`}
            aria-pressed={activeTool === tool.tool}
            onClick={() => onSelect(activeTool === tool.tool ? "" : tool.tool)}
          >
            <span className="mcp-tool-card-head">
              <span className="mcp-tool-title">{tool.title}</span>
              <PolicyBadge policy={tool.recommendedPolicy} />
            </span>
            <code className="mcp-tool-name">{tool.tool}</code>
            <span className="mcp-tool-stats">
              <span>
                <strong>{tool.calls.toLocaleString()}</strong> calls
              </span>
              <span className={tool.errors > 0 ? "mcp-tool-errors" : undefined}>
                <strong>{tool.errors.toLocaleString()}</strong> errors
                {tool.errors > 0 && ` (${Math.round(errorRate * 100)}%)`}
              </span>
              {tool.avgDurationMs !== null && (
                <span>avg {formatDuration(tool.avgDurationMs)}</span>
              )}
            </span>
            <span className="mcp-tool-last" title={formatAbsolute(tool.lastAt)}>
              Last used {formatRelative(tool.lastAt, now)}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function CallRow({ call, now }: { call: McpCall; now: number }) {
  const [open, setOpen] = useState(false);
  const claude = isClaudeTool(call.tool);
  const hasOutput = call.output !== null && call.output !== undefined;
  return (
    <li className={`mcp-call mcp-call-${call.status} ${open ? "open" : ""}`}>
      <div className="mcp-call-head">
        <button
          type="button"
          className="mcp-call-toggle"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          <span className="mcp-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>
          <span className="mcp-call-main">
            <span className="mcp-call-title">{call.title}</span>
            <code className="mcp-tool-name">{call.tool}</code>
          </span>
          <span className="mcp-call-meta">
            <CallStatusPill status={call.status} />
            <PolicyBadge policy={call.recommendedPolicy} />
            <span className="mcp-call-duration">
              {call.durationMs !== null
                ? formatDuration(call.durationMs)
                : call.status === "running"
                  ? formatDuration(now - call.startedAt)
                  : "—"}
            </span>
            <span className="mcp-call-time" title={formatAbsolute(call.startedAt)}>
              {formatRelative(call.startedAt, now)}
            </span>
          </span>
        </button>
      </div>
      {!open && call.error && <div className="mcp-call-error-line">{call.error}</div>}
      {open && (
        <div className="mcp-call-details">
          <div className="mcp-call-facts meta-row muted">
            <span>{formatAbsolute(call.startedAt)}</span>
            {call.finishedAt !== null && (
              <span>finished {formatAbsolute(call.finishedAt)}</span>
            )}
            <span>{call.readOnly ? "read-only" : "writes"}</span>
            <span className="mcp-call-id">{call.callId}</span>
          </div>
          {call.error && (
            <div className="mcp-detail-block">
              <div className="mcp-detail-label">Error</div>
              <pre className="mcp-json mcp-json-error">{call.error}</pre>
            </div>
          )}
          <div className="mcp-detail-block">
            <div className="mcp-detail-label">Input</div>
            <pre className="mcp-json">{formatJson(call.input)}</pre>
          </div>
          {hasOutput && (
            <div className="mcp-detail-block">
              <div className="mcp-detail-label">Output</div>
              <pre className="mcp-json">{formatJson(call.output)}</pre>
            </div>
          )}
          {claude && (
            <Link to="/claude" className="section-view-all">
              View in Claude Code ›
            </Link>
          )}
        </div>
      )}
    </li>
  );
}

export default function McpPage() {
  const now = useNow(15_000);
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [tool, setTool] = useState("");
  const [head, setHead] = useState<McpActivityResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [older, setOlder] = useState<{ calls: McpCall[]; nextBefore: number | null }>({
    calls: [],
    nextBefore: null,
  });
  const [headKey, setHeadKey] = useState("");
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [olderError, setOlderError] = useState<string | null>(null);

  const filterKey = `${statusFilter}:${tool}`;
  const filterOptions = {
    limit: 100,
    tool: tool || undefined,
    status: statusFilter === "all" ? undefined : statusFilter,
  } as const;

  useVisiblePoll(filterKey, () => fetchMcpActivity(filterOptions), {
    onSuccess: (response) => {
      setHead(response);
      setHeadKey(filterKey);
      setError(null);
    },
    onFailure: (err) => setError(errorMessage(err, "Failed to load MCP activity")),
  });

  // A filter change discards the first page and any older pages loaded under
  // the previous filter, and abandons an in-flight "Load older" request.
  const cancelOlderRef = useRef<() => void>(() => {});
  useEffect(() => {
    setOlder({ calls: [], nextBefore: null });
    setOlderError(null);
    setLoadingOlder(false);
    return () => cancelOlderRef.current();
  }, [filterKey]);

  // Stats and tool summaries stay visible while a new filter loads its calls.
  const callsCurrent = head !== null && headKey === filterKey;
  const calls = useMemo(() => {
    if (!head || !callsCurrent) return [];
    const seen = new Set(head.calls.map((call) => call.callId));
    return [...head.calls, ...older.calls.filter((call) => !seen.has(call.callId))];
  }, [head, older, callsCurrent]);

  const nextBefore = !callsCurrent
    ? null
    : older.calls.length > 0
      ? older.nextBefore
      : (head?.nextBefore ?? null);

  const loadOlder = () => {
    if (nextBefore === null || loadingOlder) return;
    setLoadingOlder(true);
    setOlderError(null);
    cancelOlderRef.current = forkUiRequest(
      fetchMcpActivity({ ...filterOptions, before: nextBefore }),
      {
        onSuccess: (response) => {
          setOlder((previous) => ({
            calls: [...previous.calls, ...response.calls],
            nextBefore: response.nextBefore,
          }));
          setLoadingOlder(false);
        },
        onFailure: (err) => {
          setOlderError(errorMessage(err, "Failed to load older calls"));
          setLoadingOlder(false);
        },
      },
    );
  };

  const toolOptions = head?.tools ?? [];

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>MCP Activity</h1>
          <p className="page-subtitle">
            Every tool call agents made through Omni's MCP server.{" "}
            <Link to="/claude" className="mcp-inline-link">
              Claude Code actions ›
            </Link>
          </p>
        </div>
      </div>

      {head === null && error === null && <div className="loading">Loading…</div>}
      {head === null && error !== null && (
        <div className="error">
          <div>Failed to load MCP activity</div>
          <div className="error-detail">{error}</div>
        </div>
      )}

      {head !== null && (
        <>
          {error && (
            <div className="error-inline stale-note">
              Refresh failed ({error}), showing last known state.
            </div>
          )}
          <StatTiles summary={head.summary} />

          <section className="page-section">
            <div className="section-heading-row">
              <h2 className="section-title">
                Tools <span className="section-count">{head.tools.length}</span>
              </h2>
            </div>
            <ToolSummary
              tools={head.tools}
              activeTool={tool}
              onSelect={setTool}
              now={now}
            />
          </section>

          <section className="page-section">
            <div className="section-heading-row">
              <h2 className="section-title">Calls</h2>
              <span className="muted mcp-retention">
                Keeps the latest {head.retention.maxCalls.toLocaleString()}
              </span>
            </div>
            <div className="task-toolbar">
              <div className="task-filters" role="group" aria-label="Call status">
                {STATUS_FILTERS.map(([value, label]) => (
                  <button
                    key={value}
                    type="button"
                    className={`chip-btn ${statusFilter === value ? "active" : ""}`}
                    aria-pressed={statusFilter === value}
                    onClick={() => setStatusFilter(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <select
                className="task-search mcp-tool-select"
                aria-label="Filter by tool"
                value={tool}
                onChange={(event) => setTool(event.target.value)}
              >
                <option value="">All tools</option>
                {tool && !toolOptions.some((option) => option.tool === tool) && (
                  <option value={tool}>{tool}</option>
                )}
                {toolOptions.map((option) => (
                  <option key={option.tool} value={option.tool}>
                    {option.title} ({option.tool})
                  </option>
                ))}
              </select>
            </div>

            {!callsCurrent ? (
              <div className="loading-inline">Loading calls…</div>
            ) : calls.length === 0 ? (
              <div className="muted">No calls match this view.</div>
            ) : (
              <ul className="mcp-call-list">
                {calls.map((call) => (
                  <CallRow key={call.callId} call={call} now={now} />
                ))}
              </ul>
            )}
            {olderError && <div className="error-inline">{olderError}</div>}
            {nextBefore !== null && (
              <button
                type="button"
                className="show-more-btn"
                onClick={loadOlder}
                disabled={loadingOlder}
              >
                {loadingOlder ? "Loading…" : "Load Older"}
              </button>
            )}
          </section>
        </>
      )}
    </>
  );
}
