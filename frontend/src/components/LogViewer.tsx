import { useEffect, useRef, useState } from "react";
import { fetchRunLogs, runLogStreamUrl } from "../api";
import type { RunLogLevel, RunLogLine, RunLogs, TaskRun } from "../api";
import { useNow } from "../hooks/useNow";
import { formatAbsolute } from "../utils/format";
import { StatusDot, TriggerBadge } from "./badges";
import { runDuration } from "./TaskCard";

const LEVELS: RunLogLevel[] = ["debug", "info", "warn", "error"];
const STICK_THRESHOLD_PX = 48;

function formatLogTime(t: number): string {
  const d = new Date(t);
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(
    d.getMilliseconds(),
    3,
  )}`;
}

/** "Main:LiveCheck:Twitch" → "LiveCheck:Twitch"; a bare root name stays. */
function shortLoggerName(name: string): string {
  const rest = name.split(":").slice(1).join(":");
  return rest || name;
}

/**
 * Modal log viewer for one task run. Finished runs load their persisted logs
 * once; a running task opens the per-run SSE stream and tails it live until
 * the "done" frame lands. Reconnects are safe because the stream's "init"
 * frame replaces (not appends to) the line state.
 */
export function LogViewer({
  run: initialRun,
  onClose,
}: {
  run: TaskRun;
  onClose: () => void;
}) {
  const runId = initialRun.runId;
  const startedRunning = initialRun.status === "running";
  const [run, setRun] = useState(initialRun);
  const [lines, setLines] = useState<RunLogLine[] | null>(null);
  const [dropped, setDropped] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState(startedRunning);
  const [hiddenLevels, setHiddenLevels] = useState<ReadonlySet<RunLogLevel>>(
    () => new Set(["debug"]),
  );
  const now = useNow(1000);
  const bodyRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);

  useEffect(() => {
    if (!startedRunning) {
      let cancelled = false;
      fetchRunLogs(runId)
        .then((data) => {
          if (cancelled) return;
          setRun(data.run);
          setLines(data.lines);
          setDropped(data.dropped);
        })
        .catch((err: unknown) => {
          if (cancelled) return;
          setError(err instanceof Error ? err.message : "Failed to fetch logs");
        });
      return () => {
        cancelled = true;
      };
    }

    const source = new EventSource(runLogStreamUrl(runId));
    source.addEventListener("init", (event) => {
      const data = JSON.parse((event as MessageEvent<string>).data) as RunLogs;
      setRun(data.run);
      setLines(data.lines);
      setDropped(data.dropped);
      setError(null);
    });
    source.addEventListener("line", (event) => {
      const line = JSON.parse((event as MessageEvent<string>).data) as RunLogLine;
      setLines((prev) => (prev ? [...prev, line] : [line]));
    });
    source.addEventListener("done", (event) => {
      setRun(JSON.parse((event as MessageEvent<string>).data) as TaskRun);
      setLive(false);
      source.close();
    });
    // No onerror handler: EventSource retries transient failures itself, and
    // each reconnect's "init" frame rebuilds the full state.
    return () => source.close();
  }, [runId, startedRunning]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.body.style.overflow = "";
    };
  }, [onClose]);

  const lineCount = lines?.length ?? 0;
  // Follow the tail only while the user hasn't scrolled up.
  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll on new lines
  useEffect(() => {
    const body = bodyRef.current;
    if (body && stickToBottom.current) body.scrollTop = body.scrollHeight;
  }, [lineCount, hiddenLevels]);

  const onScroll = () => {
    const body = bodyRef.current;
    if (!body) return;
    stickToBottom.current =
      body.scrollHeight - body.scrollTop - body.clientHeight < STICK_THRESHOLD_PX;
  };

  const toggleLevel = (level: RunLogLevel) => {
    setHiddenLevels((prev) => {
      const next = new Set(prev);
      if (next.has(level)) next.delete(level);
      else next.add(level);
      return next;
    });
  };

  const counts = new Map<RunLogLevel, number>();
  for (const line of lines ?? []) {
    counts.set(line.level, (counts.get(line.level) ?? 0) + 1);
  }
  const visible = (lines ?? []).filter((line) => !hiddenLevels.has(line.level));
  const hiddenCount = lineCount - visible.length;

  return (
    <div className="modal-root">
      <button
        type="button"
        className="modal-backdrop"
        onClick={onClose}
        aria-label="Close log viewer"
      />
      <div className="log-modal" role="dialog" aria-label={`Logs for ${run.taskName}`}>
        <div className="log-modal-header">
          <div className="log-modal-title">
            <StatusDot status={run.status} />
            <span className="log-modal-task">{run.taskName}</span>
            <TriggerBadge trigger={run.trigger} />
            {live && <span className="live-badge">live</span>}
          </div>
          <div className="log-modal-meta muted">
            <span>{formatAbsolute(run.startedAt)}</span>
            <span>·</span>
            <span>{runDuration(run, now)}</span>
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
        {run.error && <div className="run-error log-modal-error">{run.error}</div>}
        <div className="log-modal-controls">
          {LEVELS.map((level) => (
            <button
              key={level}
              type="button"
              className={`chip-btn log-level-chip ${hiddenLevels.has(level) ? "" : "active"}`}
              onClick={() => toggleLevel(level)}
            >
              {level}
              <span className="log-level-count">{counts.get(level) ?? 0}</span>
            </button>
          ))}
          {dropped > 0 && (
            <span className="muted log-dropped-note">
              {dropped} oldest lines dropped
            </span>
          )}
        </div>
        <div className="log-modal-body" ref={bodyRef} onScroll={onScroll}>
          {error && <div className="error-inline">Failed to load logs: {error}</div>}
          {!error && lines === null && (
            <div className="loading-inline">Loading logs…</div>
          )}
          {!error && lines !== null && lineCount === 0 && !live && (
            <div className="muted log-empty">No logs were captured for this run.</div>
          )}
          {!error && lines !== null && lineCount === 0 && live && (
            <div className="muted log-empty">Waiting for output…</div>
          )}
          {lineCount > 0 && visible.length === 0 && (
            <div className="muted log-empty">
              All {lineCount} lines are hidden by the level filter.
            </div>
          )}
          {visible.map((line, index) => (
            <div
              // Lines are append-only, so a positional key is stable.
              key={`${line.t}:${index}`}
              className={`log-line log-line-${line.level}`}
            >
              <span className="log-time">{formatLogTime(line.t)}</span>
              <span className={`log-level log-level-${line.level}`}>
                {line.level}
              </span>
              <span className="log-logger">{shortLoggerName(line.logger)}</span>
              <span className="log-msg">{line.msg}</span>
            </div>
          ))}
        </div>
        {hiddenCount > 0 && visible.length > 0 && (
          <div className="log-modal-footer muted">
            {hiddenCount} lines hidden by level filter
          </div>
        )}
      </div>
    </div>
  );
}
