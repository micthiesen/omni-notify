import type { McpCall } from "../api";

export type ClaudeActionKind =
  | "start"
  | "send"
  | "wait"
  | "result"
  | "get"
  | "read"
  | "stop"
  | "list"
  | "projects"
  | "link_status"
  | "other";

const TOOL_KINDS: Record<string, ClaudeActionKind> = {
  claude_session_start: "start",
  claude_session_send: "send",
  claude_session_wait: "wait",
  claude_session_result: "result",
  claude_session_get: "get",
  claude_session_read: "read",
  claude_session_stop: "stop",
  claude_sessions_list: "list",
  claude_projects_list: "projects",
  claude_link_status: "link_status",
};

/** Tools that act on no particular session; they render in the "Other" group. */
const SESSIONLESS: ReadonlySet<ClaudeActionKind> = new Set([
  "list",
  "projects",
  "link_status",
  "other",
]);

export const ACTION_LABELS: Record<ClaudeActionKind, string> = {
  start: "Started",
  send: "Sent",
  wait: "Waited",
  result: "Result",
  get: "Checked",
  read: "Read transcript",
  stop: "Stopped",
  list: "Listed sessions",
  projects: "Listed projects",
  link_status: "Link status",
  other: "Call",
};

export function isClaudeTool(tool: string): boolean {
  return tool.startsWith("claude_");
}

export function claudeActionKind(tool: string): ClaudeActionKind {
  return TOOL_KINDS[tool] ?? "other";
}

export function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export function stringField(value: unknown, key: string): string | null {
  const field = asRecord(value)?.[key];
  return typeof field === "string" && field.length > 0 ? field : null;
}

export function numberField(value: unknown, key: string): number | null {
  const field = asRecord(value)?.[key];
  return typeof field === "number" && Number.isFinite(field) ? field : null;
}

export function booleanField(value: unknown, key: string): boolean | null {
  const field = asRecord(value)?.[key];
  return typeof field === "boolean" ? field : null;
}

export function arrayField(value: unknown, key: string): unknown[] | null {
  const field = asRecord(value)?.[key];
  return Array.isArray(field) ? field : null;
}

/** The session snapshot an action's output carries, when it has one. */
export interface SessionSnapshot {
  sessionId: string | null;
  id: string | null;
  title: string | null;
  project: string | null;
  status: string | null;
  kind: string | null;
  revision: number | null;
}

export function outputSession(call: McpCall): SessionSnapshot | null {
  const session = asRecord(asRecord(call.output)?.session);
  if (!session) return null;
  return {
    sessionId: stringField(session, "sessionId"),
    id: stringField(session, "id"),
    title: stringField(session, "title"),
    project: stringField(session, "project"),
    status: stringField(session, "status"),
    kind: stringField(session, "kind"),
    revision: numberField(session, "revision"),
  };
}

/** Full session id recorded in the output (session.sessionId, else sessionId). */
export function outputSessionId(call: McpCall): string | null {
  return outputSession(call)?.sessionId ?? stringField(call.output, "sessionId");
}

/** The session reference the caller passed: short id, prefix, or UUID. */
export function inputSessionRef(call: McpCall): string | null {
  return stringField(call.input, "session");
}

export function shortSessionId(sessionId: string): string {
  return sessionId.slice(0, 8);
}

export interface ClaudeActionGroup {
  key: string;
  /** Full session id when known; otherwise the reference the caller used. */
  sessionId: string | null;
  title: string | null;
  project: string | null;
  status: string | null;
  kind: string | null;
  latestAt: number;
  /** Oldest first, so the timeline reads as a conversation. */
  actions: McpCall[];
  latestFailed: boolean;
}

interface KnownSession {
  sessionId: string;
  id: string | null;
}

function resolveRef(ref: string, known: KnownSession[]): string {
  const exact = known.find((k) => k.sessionId === ref || k.id === ref);
  if (exact) return exact.sessionId;
  const prefixed = known.filter(
    (k) => k.sessionId.startsWith(ref) || (k.id !== null && k.id.startsWith(ref)),
  );
  return prefixed.length === 1 ? prefixed[0].sessionId : ref;
}

/**
 * Group claude_* calls by the session they acted on. Output session ids win;
 * calls without output (errors, running calls) resolve their input reference
 * against ids seen elsewhere so a short id still joins its full-id group.
 * Input is newest first; groups are returned newest first.
 */
export function groupClaudeActions(actions: McpCall[]): {
  sessions: ClaudeActionGroup[];
  other: McpCall[];
} {
  const known: KnownSession[] = [];
  for (const call of actions) {
    const session = outputSession(call);
    const sessionId = outputSessionId(call);
    if (!sessionId || known.some((k) => k.sessionId === sessionId)) continue;
    known.push({
      sessionId,
      id: session?.id ?? stringField(call.output, "id"),
    });
  }

  const groups = new Map<string, ClaudeActionGroup>();
  const other: McpCall[] = [];
  for (const call of actions) {
    const kind = claudeActionKind(call.tool);
    if (SESSIONLESS.has(kind)) {
      other.push(call);
      continue;
    }
    const ref = inputSessionRef(call);
    const fullId = outputSessionId(call) ?? (ref ? resolveRef(ref, known) : null);
    const key = fullId ?? `call:${call.callId}`;
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        sessionId: fullId,
        title: null,
        project: null,
        status: null,
        kind: null,
        latestAt: call.startedAt,
        actions: [],
        latestFailed: call.status === "error",
      };
      groups.set(key, group);
    }
    group.actions.push(call);
    group.latestAt = Math.max(group.latestAt, call.startedAt);
    const session = outputSession(call);
    group.title ??= session?.title ?? null;
    group.project ??= session?.project ?? null;
    group.status ??= session?.status ?? null;
    group.kind ??= session?.kind ?? null;
    if (kind === "start") {
      group.title ??= stringField(call.input, "title");
      group.project ??= stringField(call.input, "project");
    }
  }

  const sessions = [...groups.values()];
  for (const group of sessions) {
    group.actions.sort((a, b) => a.startedAt - b.startedAt);
  }
  sessions.sort((a, b) => b.latestAt - a.latestAt);
  return { sessions, other };
}

export interface ErrorExplanation {
  code: string | null;
  hint: string | null;
}

const ERROR_HINTS: Array<[string, string]> = [
  ["outcome_unknown", "The Mac may have run this. Check the session before retrying."],
  [
    "not_picked_up",
    "The Mac never claimed the job before it expired, so it did not run.",
  ],
  ["not_configured", "The Mac link is not configured on the server."],
  [
    "disabled",
    "The link is switched off on the Mac. Run `omni-link enable` there to resume.",
  ],
  [
    "offline",
    "The Mac is not polling: it may be asleep, away without VPN, or the agent stopped.",
  ],
];

/** Find a link failure code in an error message and explain the known ones. */
export function explainClaudeError(error: string | null): ErrorExplanation {
  if (!error) return { code: null, hint: null };
  // Device link failures end with "(code)".
  const taggedCode = /\(([a-z][a-z_]*)\)\s*$/.exec(error.trim())?.[1];
  if (taggedCode) {
    const known = ERROR_HINTS.find(([code]) => code === taggedCode);
    return { code: taggedCode, hint: known?.[1] ?? null };
  }
  const lowered = error.toLowerCase();
  for (const [code, hint] of ERROR_HINTS) {
    if (lowered.includes(code) || lowered.includes(code.replace(/_/g, " "))) {
      return { code, hint };
    }
  }
  return { code: null, hint: null };
}

/** One-line description for compact (non-conversational) actions. */
export function summarizeCompactAction(call: McpCall): string {
  const kind = claudeActionKind(call.tool);
  const output = call.output;
  switch (kind) {
    case "list": {
      const sessions = arrayField(output, "sessions");
      const project = stringField(call.input, "project");
      const scope = project ? ` in ${project}` : "";
      return sessions
        ? `${sessions.length} session${sessions.length === 1 ? "" : "s"}${scope}`
        : `Sessions${scope}`;
    }
    case "projects": {
      const projects = arrayField(output, "projects");
      return projects
        ? `${projects.length} project${projects.length === 1 ? "" : "s"}`
        : "Projects";
    }
    case "link_status": {
      if (!asRecord(output)) return "Link status";
      const host = stringField(output, "host");
      const state = booleanField(output, "disabled")
        ? "disabled"
        : booleanField(output, "online")
          ? "online"
          : "offline";
      return host ? `${state} · ${host}` : state;
    }
    case "read": {
      const items = arrayField(output, "items");
      if (!items) return "Transcript";
      const more = booleanField(output, "hasMore") ? ", more available" : "";
      return `${items.length} item${items.length === 1 ? "" : "s"}${more}`;
    }
    case "get": {
      const session = outputSession(call);
      if (!session) return "Session";
      return session.revision === null
        ? (session.status ?? "Session")
        : `${session.status ?? "unknown"} · rev ${session.revision}`;
    }
    default:
      return ACTION_LABELS[kind];
  }
}

export function parseIsoMs(iso: string | null): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? null : ms;
}

export function formatJson(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

const TOOL_INPUT_KEYS = [
  "command",
  "file_path",
  "pattern",
  "url",
  "query",
  "description",
  "prompt",
  "skill",
  "code",
] as const;

/** One readable line for a transcript snippet, cut at `max` characters. */
export function snippet(text: string | null, max = 140): string | null {
  const line = text
    ?.split("\n")
    .map((part) => part.trim())
    .find((part) => part.length > 0);
  if (!line) return null;
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** The meaningful part of a tool call's JSON input, such as a Bash command. */
export function toolInputSummary(input: string | null): string | null {
  if (!input) return null;
  try {
    const value: unknown = JSON.parse(input);
    if (typeof value === "object" && value !== null) {
      const record = value as Record<string, unknown>;
      for (const key of TOOL_INPUT_KEYS) {
        if (typeof record[key] === "string") return snippet(record[key]);
      }
    }
  } catch {
    // Inputs are capped at 1,000 characters, so long JSON arrives cut off.
    const match = input.match(
      /"(?:command|file_path|pattern|url|query|description|prompt|skill|code)":"((?:[^"\\]|\\.)*)/,
    );
    if (match?.[1]) return snippet(match[1].replace(/\\n/g, "\n").replace(/\\"/g, '"'));
  }
  return snippet(input);
}
