import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
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
  status: "pending" | "sent";
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

export function listWorkspaceSubjects(workspaceId: string): WorkspaceSubjectData[] {
  return WorkspaceSubjectEntity.getAll()
    .filter((subject) => subject.workspaceId === workspaceId)
    .sort((a, b) => b.updatedAt - a.updatedAt);
}

export function getWorkspaceSubject(
  workspaceId: string,
  subjectId: string,
): WorkspaceSubjectData | undefined {
  return WorkspaceSubjectEntity.get({ workspaceId, subjectId });
}

export function upsertWorkspaceSubject(
  input: Omit<WorkspaceSubjectData, "createdAt" | "updatedAt"> & {
    createdAt?: number;
    updatedAt?: number;
  },
): WorkspaceSubjectData {
  const prior = getWorkspaceSubject(input.workspaceId, input.subjectId);
  const now = input.updatedAt ?? Date.now();
  const row: WorkspaceSubjectData = {
    ...input,
    createdAt: prior?.createdAt ?? input.createdAt ?? now,
    updatedAt: now,
    lastResearchedAt: input.lastResearchedAt ?? prior?.lastResearchedAt,
  };
  WorkspaceSubjectEntity.upsert(row);
  return row;
}

export function getLatestWorkspaceArtifacts(
  workspaceId: string,
  subjectId: string,
): WorkspaceArtifactRevisionData[] {
  const latest = new Map<string, WorkspaceArtifactRevisionData>();
  for (const row of WorkspaceArtifactRevisionEntity.getAll()) {
    if (row.workspaceId !== workspaceId || row.subjectId !== subjectId) continue;
    const prior = latest.get(row.artifactKey);
    if (!prior || row.createdAt > prior.createdAt) latest.set(row.artifactKey, row);
  }
  return [...latest.values()].sort((a, b) =>
    a.artifactKey.localeCompare(b.artifactKey),
  );
}

export function addWorkspaceArtifactRevision(
  input: Omit<WorkspaceArtifactRevisionData, "revisionId" | "createdAt"> & {
    createdAt?: number;
  },
): WorkspaceArtifactRevisionData | undefined {
  const prior = getLatestWorkspaceArtifacts(input.workspaceId, input.subjectId).find(
    (artifact) => artifact.artifactKey === input.artifactKey,
  );
  if (prior?.content === input.content) return undefined;
  const row: WorkspaceArtifactRevisionData = {
    ...input,
    revisionId: randomUUID(),
    createdAt: input.createdAt ?? Date.now(),
  };
  WorkspaceArtifactRevisionEntity.upsert(row);
  return row;
}

export function listWorkspaceArtifactRevisions(
  workspaceId: string,
  subjectId: string,
  artifactKey?: string,
): WorkspaceArtifactRevisionData[] {
  return WorkspaceArtifactRevisionEntity.getAll()
    .filter(
      (row) =>
        row.workspaceId === workspaceId &&
        row.subjectId === subjectId &&
        (artifactKey === undefined || row.artifactKey === artifactKey),
    )
    .sort((a, b) => b.createdAt - a.createdAt);
}

export function addWorkspaceMessage(
  input: Omit<WorkspaceMessageData, "messageId" | "createdAt"> & {
    createdAt?: number;
  },
): WorkspaceMessageData {
  const row: WorkspaceMessageData = {
    ...input,
    messageId: randomUUID(),
    createdAt: input.createdAt ?? Date.now(),
  };
  WorkspaceMessageEntity.upsert(row);
  return row;
}

export function listWorkspaceMessages(
  workspaceId: string,
  subjectId?: string,
  limit = 100,
): WorkspaceMessageData[] {
  return WorkspaceMessageEntity.getAll()
    .filter(
      (row) =>
        row.workspaceId === workspaceId &&
        (subjectId === undefined || row.subjectId === subjectId),
    )
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit)
    .reverse();
}

export function addWorkspaceSource(
  input: Omit<WorkspaceSourceData, "sourceId" | "createdAt"> & {
    sourceId?: string;
    createdAt?: number;
  },
): WorkspaceSourceData {
  const row: WorkspaceSourceData = {
    ...input,
    sourceId: input.sourceId ?? randomUUID(),
    createdAt: input.createdAt ?? Date.now(),
  };
  WorkspaceSourceEntity.upsert(row);
  return row;
}

export function getWorkspaceSource(sourceId: string): WorkspaceSourceData | undefined {
  return WorkspaceSourceEntity.get({ sourceId });
}

export function listWorkspaceSources(
  workspaceId: string,
  subjectId: string,
  limit = 100,
): WorkspaceSourceData[] {
  return WorkspaceSourceEntity.getAll()
    .filter((row) => row.workspaceId === workspaceId && row.subjectId === subjectId)
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit);
}

export function addWorkspaceAction(
  input: Omit<WorkspaceActionData, "actionId" | "createdAt" | "status">,
): { action: WorkspaceActionData; created: boolean } {
  const duplicate = WorkspaceActionEntity.getAll().find(
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
  WorkspaceActionEntity.upsert(row);
  return { action: row, created: true };
}

export function listWorkspaceActions(
  workspaceId: string,
  subjectId?: string,
): WorkspaceActionData[] {
  return WorkspaceActionEntity.getAll()
    .filter(
      (row) =>
        row.workspaceId === workspaceId &&
        (subjectId === undefined || row.subjectId === subjectId),
    )
    .sort((a, b) => b.createdAt - a.createdAt);
}

export function getWorkspaceAction(actionId: string): WorkspaceActionData | undefined {
  return WorkspaceActionEntity.get({ actionId });
}

export function setWorkspaceActionResult(
  actionId: string,
  status: Exclude<WorkspaceActionStatus, "pending">,
  result: string,
): WorkspaceActionData | undefined {
  const action = getWorkspaceAction(actionId);
  if (!action) return undefined;
  WorkspaceActionEntity.patch({ actionId }, { status, result, resolvedAt: Date.now() });
  return getWorkspaceAction(actionId);
}

export function upsertWorkspaceEmailScope(
  workspaceId: string,
  subjectId: string,
  payload: WorkspaceEmailScopePayload,
): WorkspaceEmailScopeData {
  const row = { workspaceId, subjectId, ...payload, updatedAt: Date.now() };
  WorkspaceEmailScopeEntity.upsert(row);
  return row;
}

export function getWorkspaceEmailScope(
  workspaceId: string,
  subjectId: string,
): WorkspaceEmailScopeData | undefined {
  return WorkspaceEmailScopeEntity.get({ workspaceId, subjectId });
}

export function listWorkspaceEmailScopes(
  workspaceId: string,
): WorkspaceEmailScopeData[] {
  return WorkspaceEmailScopeEntity.getAll().filter(
    (scope) => scope.workspaceId === workspaceId,
  );
}

export function listAllWorkspaceEmailScopes(): WorkspaceEmailScopeData[] {
  return WorkspaceEmailScopeEntity.getAll();
}

export function reportWorkspacePapercut(
  input: Omit<
    WorkspacePapercutData,
    | "papercutId"
    | "fingerprint"
    | "occurrences"
    | "firstSeenAt"
    | "lastSeenAt"
    | "status"
  >,
): WorkspacePapercutData {
  const fingerprint = [
    input.workspaceId,
    input.category,
    input.relatedTool ?? "",
    input.title.trim().toLowerCase(),
  ].join(":");
  const prior = WorkspacePapercutEntity.getAll().find(
    (row) => row.fingerprint === fingerprint && row.status === "open",
  );
  const now = Date.now();
  if (prior) {
    WorkspacePapercutEntity.patch(
      { papercutId: prior.papercutId },
      {
        detail: input.detail,
        runId: input.runId,
        subjectId: input.subjectId,
        occurrences: prior.occurrences + 1,
        lastSeenAt: now,
      },
    );
    return WorkspacePapercutEntity.get({ papercutId: prior.papercutId })!;
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
  WorkspacePapercutEntity.upsert(row);
  return row;
}

export function listWorkspacePapercuts(
  workspaceId?: string,
  status?: WorkspacePapercutData["status"],
): WorkspacePapercutData[] {
  return WorkspacePapercutEntity.getAll()
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
}

export function resolveWorkspacePapercut(
  papercutId: string,
  status: "addressed" | "dismissed",
  resolution: string,
): WorkspacePapercutData | undefined {
  const prior = WorkspacePapercutEntity.get({ papercutId });
  if (!prior) return undefined;
  WorkspacePapercutEntity.patch({ papercutId }, { status, resolution });
  return WorkspacePapercutEntity.get({ papercutId });
}

export function queueWorkspaceNotification(
  input: Omit<
    WorkspaceNotificationData,
    "status" | "attempts" | "createdAt" | "nextAttemptAt"
  >,
): { notification: WorkspaceNotificationData; created: boolean } {
  const prior = WorkspaceNotificationEntity.get({
    notificationId: input.notificationId,
  });
  if (prior) return { notification: prior, created: false };
  const now = Date.now();
  const notification: WorkspaceNotificationData = {
    ...input,
    status: "pending",
    attempts: 0,
    createdAt: now,
    nextAttemptAt: now,
  };
  WorkspaceNotificationEntity.upsert(notification);
  return { notification, created: true };
}

export function listDueWorkspaceNotifications(
  now = Date.now(),
  limit = 20,
): WorkspaceNotificationData[] {
  return WorkspaceNotificationEntity.getAll()
    .filter(
      (notification) =>
        notification.status === "pending" && notification.nextAttemptAt <= now,
    )
    .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt)
    .slice(0, limit);
}

export function markWorkspaceNotificationSent(notificationId: string): void {
  WorkspaceNotificationEntity.patch(
    { notificationId },
    { status: "sent", sentAt: Date.now(), lastError: undefined },
  );
}

export function markWorkspaceNotificationFailed(
  notificationId: string,
  attempts: number,
  error: string,
): void {
  const delayMs = Math.min(
    5 * 60_000 * 2 ** Math.min(Math.max(attempts - 1, 0), 6),
    6 * 60 * 60_000,
  );
  WorkspaceNotificationEntity.patch(
    { notificationId },
    {
      attempts,
      lastError: error,
      nextAttemptAt: Date.now() + delayMs,
    },
  );
}
