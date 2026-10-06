/** Validated subscription arguments. Every event takes string-valued filters. */
export type EventArguments = Readonly<Record<string, string>>;
export type EventData = Readonly<Record<string, unknown>>;

export interface EventDefinition {
  readonly name: string;
  readonly description: string;
  readonly inputSchema: Record<string, unknown>;
  readonly payloadSchema: Record<string, unknown>;
  /** The arguments in canonical form, or undefined when they are invalid. */
  readonly parseArguments: (raw: unknown) => EventArguments | undefined;
  /** Whether an event with this payload belongs to a subscription's arguments. */
  readonly matches: (args: EventArguments, data: EventData) => boolean;
}

export const EMAIL_RECEIVED = "email.received";
export const CLAUDE_TURN_FINISHED = "claude.session.turn_finished";

const FOLDERS = new Set(["inbox", "archive"]);
const PROJECT = /^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$/;

function plainObject(raw: unknown): Record<string, unknown> | undefined {
  return typeof raw === "object" && raw !== null && !Array.isArray(raw)
    ? (raw as Record<string, unknown>)
    : undefined;
}

const emailReceived: EventDefinition = {
  name: EMAIL_RECEIVED,
  description:
    "A new iCloud message arrived in the selected Inbox or Archive mailbox. Read full mail with email tools using messageId.",
  inputSchema: {
    type: "object",
    properties: { folder: { type: "string", enum: ["inbox", "archive"] } },
    required: ["folder"],
    additionalProperties: false,
  },
  payloadSchema: {
    type: "object",
    properties: {
      messageId: { type: "string" },
      folder: { type: "string", enum: ["inbox", "archive"] },
      uidValidity: { type: "string" },
      uid: { type: "integer" },
    },
    required: ["messageId", "folder", "uidValidity", "uid"],
    additionalProperties: false,
  },
  parseArguments: (raw): EventArguments | undefined => {
    const args = plainObject(raw);
    if (!args || Object.keys(args).length !== 1) return undefined;
    return typeof args.folder === "string" && FOLDERS.has(args.folder)
      ? { folder: args.folder }
      : undefined;
  },
  matches: (args, data) => data.folder === args.folder,
};

const claudeTurnFinished: EventDefinition = {
  name: CLAUDE_TURN_FINISHED,
  description:
    "A Claude Code session on the Claude Code host finished its turn or stopped. Omni checks the host about every 15 seconds while a subscription is active. Read the result with claude_session_get (includeResult) using sessionId.",
  inputSchema: {
    type: "object",
    properties: {
      project: {
        type: "string",
        description: "Only sessions in this project (see claude_link_status)",
      },
    },
    additionalProperties: false,
  },
  payloadSchema: {
    type: "object",
    properties: {
      sessionId: { type: "string" },
      id: { type: ["string", "null"] },
      project: { type: ["string", "null"] },
      status: { type: "string" },
      revision: { type: "integer" },
    },
    required: ["sessionId", "id", "project", "status", "revision"],
    additionalProperties: false,
  },
  parseArguments: (raw): EventArguments | undefined => {
    const args = plainObject(raw);
    if (!args) return undefined;
    const { project, ...rest } = args;
    if (Object.keys(rest).length > 0) return undefined;
    if (project === undefined) return {};
    return typeof project === "string" && PROJECT.test(project)
      ? { project }
      : undefined;
  },
  matches: (args, data) => !args.project || data.project === args.project,
};

export const EVENT_DEFINITIONS: readonly EventDefinition[] = [
  emailReceived,
  claudeTurnFinished,
];

export function eventDefinition(name: string): EventDefinition | undefined {
  return EVENT_DEFINITIONS.find((definition) => definition.name === name);
}

/** Stable JSON with sorted keys; subscription identity depends on it. */
export function canonicalEventArguments(args: EventArguments): string {
  return JSON.stringify(
    Object.fromEntries(Object.entries(args).sort(([a], [b]) => a.localeCompare(b))),
  );
}
