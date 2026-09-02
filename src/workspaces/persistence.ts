import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore, type DocstoreSync } from "@micthiesen/mitools/docstore";
import { Clock, Effect, Option } from "effect";
import type {
  WorkspaceActionStatus,
  WorkspaceActionType,
  WorkspaceArtifactKind,
  WorkspaceEmailScopePayload,
  WorkspaceSubjectStatus,
} from "./types.js";

export interface WorkspaceSubjectData {
  workspaceId: string;
  subjectId: string;
  title: string;
  status: WorkspaceSubjectStatus;
  summary: string;
  createdAt: number;
  updatedAt: number;
  lastResearchedAt?: number;
}

export interface WorkspaceArtifactRevisionData {
  revisionId: string;
  workspaceId: string;
  subjectId: string;
  artifactKey: string;
  kind: WorkspaceArtifactKind;
  content: string;
  summary: string;
  createdAt: number;
  runId?: string;
}

export interface WorkspaceMessageData {
  messageId: string;
  workspaceId: string;
  subjectId?: string;
  role: "user" | "assistant" | "system";
  text: string;
  createdAt: number;
  runId?: string;
}

export interface WorkspaceSourceData {
  sourceId: string;
  workspaceId: string;
  subjectId: string;
  kind: "web" | "email";
  title: string;
  url?: string;
  excerpt: string;
  emailId?: string;
  createdAt: number;
  runId?: string;
  triggeredAt?: number;
}

export interface WorkspaceActionData {
  actionId: string;
  workspaceId: string;
  subjectId: string;
  type: WorkspaceActionType;
  status: WorkspaceActionStatus;
  title: string;
  description: string;
  payload: string;
  createdAt: number;
  resolvedAt?: number;
  result?: string;
  runId?: string;
}

export interface WorkspaceEmailScopeData extends WorkspaceEmailScopePayload {
  workspaceId: string;
  subjectId: string;
  updatedAt: number;
}

export type WorkspacePapercutCategory =
  | "missing-capability"
  | "poor-source-data"
  | "integration-friction"
  | "workflow-gap"
  | "prompt-problem"
  | "ui-gap";

export interface WorkspacePapercutData {
  papercutId: string;
  workspaceId: string;
  subjectId?: string;
  runId?: string;
  category: WorkspacePapercutCategory;
  title: string;
  detail: string;
  relatedTool?: string;
  fingerprint: string;
  occurrences: number;
  firstSeenAt: number;
  lastSeenAt: number;
  status: "open" | "addressed" | "dismissed";
  resolution?: string;
}

export interface WorkspaceNotificationData {
  notificationId: string;
  workspaceId: string;
  subjectId: string;
  title: string;
  message: string;
  url: string;
  urlTitle: string;
  status: "pending" | "sending" | "sent" | "unknown";
  attempts: number;
  createdAt: number;
  nextAttemptAt: number;
  sentAt?: number;
  lastError?: string;
}

export const WorkspaceSubjectEntity = new Entity<
  WorkspaceSubjectData,
  ["workspaceId", "subjectId"]
>("workspace-subject", ["workspaceId", "subjectId"]);

export const WorkspaceArtifactRevisionEntity = new Entity<
  WorkspaceArtifactRevisionData,
  ["revisionId"]
>("workspace-artifact-revision", ["revisionId"]);

export const WorkspaceMessageEntity = new Entity<WorkspaceMessageData, ["messageId"]>(
  "workspace-message",
  ["messageId"],
);

export const WorkspaceSourceEntity = new Entity<WorkspaceSourceData, ["sourceId"]>(
  "workspace-source",
  ["sourceId"],
);

export const WorkspaceActionEntity = new Entity<WorkspaceActionData, ["actionId"]>(
  "workspace-action",
  ["actionId"],
);

export const WorkspaceEmailScopeEntity = new Entity<
  WorkspaceEmailScopeData,
  ["workspaceId", "subjectId"]
>("workspace-email-scope", ["workspaceId", "subjectId"]);

export const WorkspacePapercutEntity = new Entity<
  WorkspacePapercutData,
  ["papercutId"]
>("workspace-papercut", ["papercutId"]);

export const WorkspaceNotificationEntity = new Entity<
  WorkspaceNotificationData,
  ["notificationId"]
>("workspace-notification", ["notificationId"]);

type WorkspaceEntity<Data, Key> = {
  readonly name: string;
  readonly _data?: (value: Data) => Data;
  getPk(key: Key): string;
};

export interface WorkspaceTransaction {
  get<Data, Key>(entity: WorkspaceEntity<Data, Key>, key: Key): Data | undefined;
  all<Data, Key>(entity: WorkspaceEntity<Data, Key>): Data[];
  upsert<Data, Key>(entity: WorkspaceEntity<Data, Key>, key: Key, data: Data): void;
  patch<Data, Key>(
    entity: WorkspaceEntity<Data, Key>,
    key: Key,
    partial: Partial<Data>,
  ): Data | undefined;
}

/** Execute a complete workspace output commit in one SQLite transaction. */
export const applyWorkspaceTransaction = Effect.fn("Workspace.transaction")(function* <
  A,
>(apply: (transaction: WorkspaceTransaction, now: number) => A) {
  const docstore = yield* Docstore;
  const now = yield* Clock.currentTimeMillis;
  return yield* docstore.transaction("commit workspace output", (raw) => {
    const transaction = makeWorkspaceTransaction(raw, now);
    return apply(transaction, now);
  });
});

function makeWorkspaceTransaction(
  raw: DocstoreSync,
  now: number,
): WorkspaceTransaction {
  const get = <Data, Key>(entity: WorkspaceEntity<Data, Key>, key: Key) => {
    const row = raw.getRawRow(entity.getPk(key), now);
    return row ? decodeDoc<Data>(row.data) : undefined;
  };
  const upsert = <Data, Key>(
    entity: WorkspaceEntity<Data, Key>,
    key: Key,
    data: Data,
  ) => {
    raw.upsertDoc(
      entity.getPk(key),
      data,
      { entity: entity.name, version: 0, expiresAt: null, updatedAt: now },
      now,
    );
  };
  return {
    get,
    all: <Data, Key>(entity: WorkspaceEntity<Data, Key>) =>
      raw
        .getRawRowsByPrefix(`$${entity.name}#`)
        .filter((row) => row.expires_at === null || row.expires_at > now)
        .map((row) => decodeDoc<Data>(row.data)),
    upsert,
    patch: <Data, Key>(
      entity: WorkspaceEntity<Data, Key>,
      key: Key,
      partial: Partial<Data>,
    ) => {
      const current = get(entity, key);
      if (!current) return undefined;
      const next = { ...current, ...partial };
      upsert(entity, key, next);
      return next;
    },
  };
}

export const listWorkspaceSubjects = Effect.fn("Workspace.listSubjects")(function* (
  workspaceId: string,
) {
  return (yield* WorkspaceSubjectEntity.getAll())
    .filter((subject) => subject.workspaceId === workspaceId)
    .sort((a, b) => b.updatedAt - a.updatedAt);
});

export const getWorkspaceSubject = Effect.fn("Workspace.getSubject")(function* (
  workspaceId: string,
  subjectId: string,
) {
  return Option.getOrUndefined(
    yield* WorkspaceSubjectEntity.get({ workspaceId, subjectId }),
  );
});

export const upsertWorkspaceSubject = Effect.fn("Workspace.upsertSubject")(function* (
  input: Omit<WorkspaceSubjectData, "createdAt" | "updatedAt"> & {
    createdAt?: number;
    updatedAt?: number;
  },
) {
  const prior = yield* getWorkspaceSubject(input.workspaceId, input.subjectId);
  const now = input.updatedAt ?? Date.now();
  const row: WorkspaceSubjectData = {
    ...input,
    createdAt: prior?.createdAt ?? input.createdAt ?? now,
    updatedAt: now,
    lastResearchedAt: input.lastResearchedAt ?? prior?.lastResearchedAt,
  };
  yield* WorkspaceSubjectEntity.upsert(row);
  return row;
});

export const getLatestWorkspaceArtifacts = Effect.fn("Workspace.latestArtifacts")(
  function* (workspaceId: string, subjectId: string) {
    const latest = new Map<string, WorkspaceArtifactRevisionData>();
    for (const row of yield* WorkspaceArtifactRevisionEntity.getAll()) {
      if (row.workspaceId !== workspaceId || row.subjectId !== subjectId) continue;
      const prior = latest.get(row.artifactKey);
      if (!prior || row.createdAt > prior.createdAt) latest.set(row.artifactKey, row);
    }
    return [...latest.values()].sort((a, b) =>
      a.artifactKey.localeCompare(b.artifactKey),
    );
  },
);

export const addWorkspaceArtifactRevision = Effect.fn("Workspace.addArtifactRevision")(
  function* (
    input: Omit<WorkspaceArtifactRevisionData, "revisionId" | "createdAt"> & {
      createdAt?: number;
    },
  ) {
    const prior = (yield* getLatestWorkspaceArtifacts(
      input.workspaceId,
      input.subjectId,
    )).find((artifact) => artifact.artifactKey === input.artifactKey);
    if (prior?.content === input.content) return undefined;
    const row: WorkspaceArtifactRevisionData = {
      ...input,
      revisionId: randomUUID(),
      createdAt: input.createdAt ?? Date.now(),
    };
    yield* WorkspaceArtifactRevisionEntity.upsert(row);
    return row;
  },
);

export const listWorkspaceArtifactRevisions = Effect.fn(
  "Workspace.listArtifactRevisions",
)(function* (workspaceId: string, subjectId: string, artifactKey?: string) {
  return (yield* WorkspaceArtifactRevisionEntity.getAll())
    .filter(
      (row) =>
        row.workspaceId === workspaceId &&
        row.subjectId === subjectId &&
        (artifactKey === undefined || row.artifactKey === artifactKey),
    )
    .sort((a, b) => b.createdAt - a.createdAt);
});

export const addWorkspaceMessage = Effect.fn("Workspace.addMessage")(function* (
  input: Omit<WorkspaceMessageData, "messageId" | "createdAt"> & {
    createdAt?: number;
  },
) {
  const row: WorkspaceMessageData = {
    ...input,
    messageId: randomUUID(),
    createdAt: input.createdAt ?? Date.now(),
  };
  yield* WorkspaceMessageEntity.upsert(row);
  return row;
});

export const assignWorkspaceMessageSubject = Effect.fn(
  "Workspace.assignMessageSubject",
)(function* (messageId: string, subjectId: string) {
  yield* WorkspaceMessageEntity.patch({ messageId }, { subjectId });
});

export const listWorkspaceMessages = Effect.fn("Workspace.listMessages")(function* (
  workspaceId: string,
  subjectId?: string,
  limit = 100,
) {
  return (yield* WorkspaceMessageEntity.getAll())
    .filter(
      (row) =>
        row.workspaceId === workspaceId &&
        (subjectId === undefined || row.subjectId === subjectId),
    )
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit)
    .reverse();
});

export const addWorkspaceSource = Effect.fn("Workspace.addSource")(function* (
  input: Omit<WorkspaceSourceData, "sourceId" | "createdAt"> & {
    sourceId?: string;
    createdAt?: number;
  },
) {
  const row: WorkspaceSourceData = {
    ...input,
    sourceId: input.sourceId ?? randomUUID(),
    createdAt: input.createdAt ?? Date.now(),
  };
  yield* WorkspaceSourceEntity.upsert(row);
  return row;
});

export const getWorkspaceSource = Effect.fn("Workspace.getSource")(function* (
  sourceId: string,
) {
  return Option.getOrUndefined(yield* WorkspaceSourceEntity.get({ sourceId }));
});

export const markWorkspaceSourceTriggered = Effect.fn("Workspace.markSourceTriggered")(
  function* (sourceId: string) {
    yield* WorkspaceSourceEntity.patch({ sourceId }, { triggeredAt: Date.now() });
  },
);

export const markWorkspaceSourcesTriggered = Effect.fn(
  "Workspace.markSourcesTriggered",
)(function* (sourceIds: string[]) {
  const triggeredAt = Date.now();
  yield* Effect.forEach(
    sourceIds,
    (sourceId) => WorkspaceSourceEntity.patch({ sourceId }, { triggeredAt }),
    { discard: true },
  );
});

export const listWorkspaceSources = Effect.fn("Workspace.listSources")(function* (
  workspaceId: string,
  subjectId: string,
  limit = 100,
) {
  return (yield* WorkspaceSourceEntity.getAll())
    .filter((row) => row.workspaceId === workspaceId && row.subjectId === subjectId)
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit);
});

export const addWorkspaceAction = Effect.fn("Workspace.addAction")(function* (
  input: Omit<WorkspaceActionData, "actionId" | "createdAt" | "status">,
) {
  const duplicate = (yield* WorkspaceActionEntity.getAll()).find(
    (row) =>
      row.workspaceId === input.workspaceId &&
      row.subjectId === input.subjectId &&
      row.type === input.type &&
      row.status === "pending" &&
      row.payload === input.payload,
  );
  if (duplicate) return { action: duplicate, created: false };
  const row: WorkspaceActionData = {
    ...input,
    actionId: randomUUID(),
    status: "pending",
    createdAt: Date.now(),
  };
  yield* WorkspaceActionEntity.upsert(row);
  return { action: row, created: true };
});

export const listWorkspaceActions = Effect.fn("Workspace.listActions")(function* (
  workspaceId: string,
  subjectId?: string,
) {
  return (yield* WorkspaceActionEntity.getAll())
    .filter(
      (row) =>
        row.workspaceId === workspaceId &&
        (subjectId === undefined || row.subjectId === subjectId),
    )
    .sort((a, b) => b.createdAt - a.createdAt);
});

export const getWorkspaceAction = Effect.fn("Workspace.getAction")(function* (
  actionId: string,
) {
  return Option.getOrUndefined(yield* WorkspaceActionEntity.get({ actionId }));
});

export const setWorkspaceActionResult = Effect.fn("Workspace.setActionResult")(
  function* (
    actionId: string,
    status: Exclude<WorkspaceActionStatus, "pending">,
    result: string,
  ) {
    const action = yield* getWorkspaceAction(actionId);
    if (!action) return undefined;
    return Option.getOrUndefined(
      yield* WorkspaceActionEntity.patch(
        { actionId },
        { status, result, resolvedAt: Date.now() },
      ),
    );
  },
);

export const upsertWorkspaceEmailScope = Effect.fn("Workspace.upsertEmailScope")(
  function* (
    workspaceId: string,
    subjectId: string,
    payload: WorkspaceEmailScopePayload,
  ) {
    const row = { workspaceId, subjectId, ...payload, updatedAt: Date.now() };
    yield* WorkspaceEmailScopeEntity.upsert(row);
    return row;
  },
);

export const getWorkspaceEmailScope = Effect.fn("Workspace.getEmailScope")(function* (
  workspaceId: string,
  subjectId: string,
) {
  return Option.getOrUndefined(
    yield* WorkspaceEmailScopeEntity.get({ workspaceId, subjectId }),
  );
});

export const listWorkspaceEmailScopes = Effect.fn("Workspace.listEmailScopes")(
  function* (workspaceId: string) {
    return (yield* WorkspaceEmailScopeEntity.getAll()).filter(
      (scope) => scope.workspaceId === workspaceId,
    );
  },
);

export const listAllWorkspaceEmailScopes = Effect.fn("Workspace.listAllEmailScopes")(
  function* () {
    return yield* WorkspaceEmailScopeEntity.getAll();
  },
);

export const reportWorkspacePapercut = Effect.fn("Workspace.reportPapercut")(function* (
  input: Omit<
    WorkspacePapercutData,
    | "papercutId"
    | "fingerprint"
    | "occurrences"
    | "firstSeenAt"
    | "lastSeenAt"
    | "status"
  >,
) {
  const fingerprint = [
    input.workspaceId,
    input.category,
    input.relatedTool ?? "",
    input.title.trim().toLowerCase(),
  ].join(":");
  const prior = (yield* WorkspacePapercutEntity.getAll()).find(
    (row) => row.fingerprint === fingerprint && row.status === "open",
  );
  const now = Date.now();
  if (prior) {
    const updated = yield* WorkspacePapercutEntity.patch(
      { papercutId: prior.papercutId },
      {
        detail: input.detail,
        runId: input.runId,
        subjectId: input.subjectId,
        occurrences: prior.occurrences + 1,
        lastSeenAt: now,
      },
    );
    return Option.getOrThrow(updated);
  }
  const row: WorkspacePapercutData = {
    ...input,
    papercutId: randomUUID(),
    fingerprint,
    occurrences: 1,
    firstSeenAt: now,
    lastSeenAt: now,
    status: "open",
  };
  yield* WorkspacePapercutEntity.upsert(row);
  return row;
});

export const listWorkspacePapercuts = Effect.fn("Workspace.listPapercuts")(function* (
  workspaceId?: string,
  status?: WorkspacePapercutData["status"],
) {
  return (yield* WorkspacePapercutEntity.getAll())
    .filter(
      (row) =>
        (workspaceId === undefined || row.workspaceId === workspaceId) &&
        (status === undefined || row.status === status),
    )
    .sort(
      (a, b) =>
        Number(b.status === "open") - Number(a.status === "open") ||
        b.occurrences - a.occurrences ||
        b.lastSeenAt - a.lastSeenAt,
    );
});

export const resolveWorkspacePapercut = Effect.fn("Workspace.resolvePapercut")(
  function* (
    papercutId: string,
    status: "addressed" | "dismissed",
    resolution: string,
  ) {
    return Option.getOrUndefined(
      yield* WorkspacePapercutEntity.patch({ papercutId }, { status, resolution }),
    );
  },
);

export const queueWorkspaceNotification = Effect.fn("Workspace.queueNotification")(
  function* (
    input: Omit<
      WorkspaceNotificationData,
      "status" | "attempts" | "createdAt" | "nextAttemptAt"
    >,
  ) {
    const prior = Option.getOrUndefined(
      yield* WorkspaceNotificationEntity.get({
        notificationId: input.notificationId,
      }),
    );
    if (prior) return { notification: prior, created: false };
    const now = Date.now();
    const notification: WorkspaceNotificationData = {
      ...input,
      status: "pending",
      attempts: 0,
      createdAt: now,
      nextAttemptAt: now,
    };
    yield* WorkspaceNotificationEntity.upsert(notification);
    return { notification, created: true };
  },
);

export const listDueWorkspaceNotifications = Effect.fn(
  "Workspace.listDueNotifications",
)(function* (now = Date.now(), limit = 20) {
  return (yield* WorkspaceNotificationEntity.getAll())
    .filter(
      (notification) =>
        notification.status === "sending" ||
        (notification.status === "pending" && notification.nextAttemptAt <= now),
    )
    .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt)
    .slice(0, limit);
});

export const markWorkspaceNotificationSent = Effect.fn(
  "Workspace.markNotificationSent",
)(function* (notificationId: string) {
  yield* WorkspaceNotificationEntity.patch(
    { notificationId },
    { status: "sent", sentAt: Date.now(), lastError: undefined },
  );
});

/** Reserve one at-most-once provider attempt before leaving SQLite. */
export const markWorkspaceNotificationSending = Effect.fn(
  "Workspace.markNotificationSending",
)(function* (notificationId: string, attempts: number) {
  yield* WorkspaceNotificationEntity.patch(
    { notificationId },
    { status: "sending", attempts, lastError: undefined },
  );
});

export const markWorkspaceNotificationUnknown = Effect.fn(
  "Workspace.markNotificationUnknown",
)(function* (notificationId: string) {
  yield* WorkspaceNotificationEntity.patch(
    { notificationId },
    {
      status: "unknown",
      lastError: "Provider attempt outcome is unknown; suppressed automatic resend",
    },
  );
});

export const markWorkspaceNotificationFailed = Effect.fn(
  "Workspace.markNotificationFailed",
)(function* (notificationId: string, attempts: number, error: string) {
  const delayMs = Math.min(
    5 * 60_000 * 2 ** Math.min(Math.max(attempts - 1, 0), 6),
    6 * 60 * 60_000,
  );
  yield* WorkspaceNotificationEntity.patch(
    { notificationId },
    {
      status: "pending",
      attempts,
      lastError: error,
      nextAttemptAt: Date.now() + delayMs,
    },
  );
});
