import type { NamedLogger } from "@micthiesen/mitools/logging";
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Effect, Exit } from "effect";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import type {
  WorkspaceActionData,
  WorkspaceMessageData,
  WorkspaceSourceData,
  WorkspaceSubjectData,
} from "./persistence.js";
import type { WorkspaceDefinition } from "./types.js";

const state = vi.hoisted(() => ({
  subjects: new Map<string, WorkspaceSubjectData>(),
  artifacts: [] as Array<Record<string, unknown>>,
  actions: [] as WorkspaceActionData[],
  messages: [] as WorkspaceMessageData[],
  sources: [] as WorkspaceSourceData[],
  papercuts: [] as Array<Record<string, unknown>>,
  notifications: [] as Array<Record<string, unknown>>,
  actionCreated: true,
  deliver: vi.fn().mockResolvedValue(true),
}));

vi.mock("./notifications.js", () => ({
  deliverWorkspaceNotification: state.deliver,
  deliverWorkspaceNotificationEffect: (...args: unknown[]) =>
    Effect.promise(() => state.deliver(...args)),
}));

vi.mock("./persistence.js", () => ({
  WorkspaceActionEntity: { name: "workspace-action" },
  WorkspaceArtifactRevisionEntity: { name: "workspace-artifact-revision" },
  WorkspaceMessageEntity: { name: "workspace-message" },
  WorkspaceNotificationEntity: { name: "workspace-notification" },
  WorkspaceSourceEntity: { name: "workspace-source" },
  WorkspaceSubjectEntity: { name: "workspace-subject" },
  applyWorkspaceTransaction: <A>(
    apply: (
      transaction: {
        get: (entity: { name: string }, key: Record<string, string>) => unknown;
        all: (entity: { name: string }) => unknown[];
        upsert: (
          entity: { name: string },
          key: Record<string, string>,
          data: Record<string, unknown>,
        ) => void;
        patch: (
          entity: { name: string },
          key: Record<string, string>,
          partial: Record<string, unknown>,
        ) => unknown;
      },
      now: number,
    ) => A,
  ) =>
    Effect.sync(() => {
      type StoredRow = Record<string, unknown>;
      const rows = (name: string): StoredRow[] => {
        if (name === "workspace-artifact-revision") return state.artifacts;
        if (name === "workspace-action") return state.actions as unknown as StoredRow[];
        if (name === "workspace-message")
          return state.messages as unknown as StoredRow[];
        if (name === "workspace-source") return state.sources as unknown as StoredRow[];
        if (name === "workspace-notification") return state.notifications;
        if (name === "workspace-subject")
          return [...state.subjects.values()] as unknown as StoredRow[];
        return [];
      };
      const transaction = {
        get: (entity: { name: string }, key: Record<string, string>) =>
          rows(entity.name).find((row) =>
            Object.entries(key).every(([field, value]) => row[field] === value),
          ),
        all: (entity: { name: string }) => rows(entity.name),
        upsert: (
          entity: { name: string },
          key: Record<string, string>,
          data: Record<string, unknown>,
        ) => {
          if (entity.name === "workspace-subject") {
            const subject = data as unknown as WorkspaceSubjectData;
            state.subjects.set(`${subject.workspaceId}:${subject.subjectId}`, subject);
            return;
          }
          const collection = rows(entity.name);
          const index = collection.findIndex((row) =>
            Object.entries(key).every(([field, value]) => row[field] === value),
          );
          if (index >= 0) collection[index] = data;
          else collection.push(data);
        },
        patch: (
          entity: { name: string },
          key: Record<string, string>,
          partial: Record<string, unknown>,
        ) => {
          const row = transaction.get(entity, key) as
            | Record<string, unknown>
            | undefined;
          if (row) Object.assign(row, partial);
          return row;
        },
      };
      return apply(transaction, 1);
    }),
  addWorkspaceAction: (
    input: Omit<WorkspaceActionData, "actionId" | "status" | "createdAt">,
  ) => {
    const action: WorkspaceActionData = {
      ...input,
      actionId: "action-1",
      status: "pending",
      createdAt: 1,
    };
    if (state.actionCreated) state.actions.push(action);
    return { action, created: state.actionCreated };
  },
  addWorkspaceArtifactRevision: (input: Record<string, unknown>) => {
    state.artifacts.push(input);
    return input;
  },
  addWorkspaceMessage: (
    input: Omit<WorkspaceMessageData, "messageId" | "createdAt">,
  ) => {
    const message: WorkspaceMessageData = {
      ...input,
      messageId: `message-${state.messages.length}`,
      createdAt: 1,
    };
    state.messages.push(message);
    return message;
  },
  assignWorkspaceMessageSubject: (messageId: string, subjectId: string) => {
    const message = state.messages.find((item) => item.messageId === messageId);
    if (message) message.subjectId = subjectId;
  },
  addWorkspaceSource: (input: Omit<WorkspaceSourceData, "sourceId" | "createdAt">) => {
    const source: WorkspaceSourceData = {
      ...input,
      sourceId: `source-${state.sources.length}`,
      createdAt: 1,
    };
    state.sources.push(source);
    return source;
  },
  getLatestWorkspaceArtifacts: () => Effect.succeed([]),
  getWorkspaceSubject: (workspaceId: string, subjectId: string) =>
    Effect.succeed(state.subjects.get(`${workspaceId}:${subjectId}`)),
  listWorkspaceEmailScopes: () => Effect.succeed([]),
  listWorkspaceMessages: () => Effect.succeed([]),
  listWorkspaceSources: () => Effect.succeed([]),
  listWorkspaceSubjects: () => Effect.succeed([...state.subjects.values()]),
  queueWorkspaceNotification: (input: Record<string, unknown>) => {
    const notification = { ...input, status: "pending", attempts: 0 };
    state.notifications.push(notification);
    return Effect.succeed({ notification, created: true });
  },
  reportWorkspacePapercut: (input: Record<string, unknown>) => {
    state.papercuts.push(input);
    return Effect.succeed({ ...input, papercutId: "papercut-1", occurrences: 1 });
  },
  upsertWorkspaceSubject: (input: WorkspaceSubjectData) => {
    const subject = { ...input, createdAt: input.createdAt ?? 1, updatedAt: 1 };
    state.subjects.set(`${subject.workspaceId}:${subject.subjectId}`, subject);
    return subject;
  },
}));

import { applyWorkspaceOutputEffect, normalizeWorkspaceWebUrl } from "./engine.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

const definition: WorkspaceDefinition = {
  id: "purchase-research",
  title: "Purchase Research",
  description: "",
  subjectLabel: "Purchase",
  subjectLabelPlural: "Purchases",
  taskName: "PurchaseResearch",
  schedule: "0 0 9 * * 0",
  instructions: "",
  artifacts: [{ key: "brief", title: "Brief", kind: "markdown", instructions: "" }],
};
const logger = {
  info: vi.fn(() => Effect.void),
  warn: vi.fn(() => Effect.void),
} as unknown as NamedLogger;

type Output = Parameters<typeof applyWorkspaceOutputEffect>[1];

function output(subjectId = "new-1"): Output {
  return {
    response: "I created the dossier.",
    subjects: [
      {
        subject_id: subjectId,
        title: "Camera",
        status: "active",
        summary: "Camera research",
        artifact_updates: [
          { key: "brief", content: "Requirements", summary: "Initial brief" },
        ],
      },
    ],
    sources: [
      {
        subject_id: subjectId,
        title: "Unsafe result",
        url: "javascript:alert(1)",
        excerpt: "Evidence",
      },
    ],
    proposals: [
      {
        type: "email_scope",
        subject_id: subjectId,
        title: "Watch Sale Emails",
        description: "Watch one retailer.",
        senders: ["alerts@example.com"],
        domains: [],
        subject_keywords: [],
        body_keywords: [],
        event: null,
      },
    ],
    notification: null,
  };
}

describe("workspace output application", () => {
  beforeEach(() => {
    state.subjects.clear();
    state.artifacts.length = 0;
    state.actions.length = 0;
    state.messages.length = 0;
    state.sources.length = 0;
    state.papercuts.length = 0;
    state.notifications.length = 0;
    state.actionCreated = true;
    state.deliver.mockClear();
  });

  it("maps one new subject across artifacts, sources, messages, and actions", async () => {
    const result = await runtime.run(
      applyWorkspaceOutputEffect(
        definition,
        output(),
        { trigger: "message", message: "Find me a camera" },
        logger,
      ),
    );

    const subject = [...state.subjects.values()][0];
    expect(subject?.subjectId).toBeTruthy();
    expect(state.artifacts[0]?.subjectId).toBe(subject?.subjectId);
    expect(state.sources[0]?.subjectId).toBe(subject?.subjectId);
    expect(state.sources[0]?.url).toBeUndefined();
    expect(state.actions[0]?.subjectId).toBe(subject?.subjectId);
    expect(state.messages).toHaveLength(2);
    expect(
      state.messages.every((message) => message.subjectId === subject?.subjectId),
    ).toBe(true);
    expect(result).toMatchObject({ updatedSubjects: 1, createdActions: 1 });
    expect(state.deliver).toHaveBeenCalledTimes(1);
  });

  it("attaches a pre-model user message to the subject without duplicating it", async () => {
    state.messages.push({
      messageId: "persisted-user-message",
      workspaceId: definition.id,
      role: "user",
      text: "Find me a camera",
      createdAt: 1,
      runId: "run-1",
    });

    await runtime.run(
      applyWorkspaceOutputEffect(
        definition,
        output(),
        { trigger: "message", message: "Find me a camera" },
        logger,
        "persisted-user-message",
      ),
    );

    const subject = [...state.subjects.values()][0];
    expect(state.messages).toHaveLength(2);
    expect(state.messages[0]).toMatchObject({
      messageId: "persisted-user-message",
      subjectId: subject?.subjectId,
    });
  });

  it("rejects hallucinated subject IDs instead of allocating a dossier", async () => {
    await expect(
      runtime.run(
        applyWorkspaceOutputEffect(
          definition,
          output("typo-subject"),
          { trigger: "message", message: "Update it" },
          logger,
        ),
      ),
    ).rejects.toThrow('unknown subject_id "typo-subject"');
    expect(state.subjects.size).toBe(0);
  });

  it("does not let a subject-scoped run update another dossier", async () => {
    state.subjects.set("purchase-research:camera", {
      workspaceId: "purchase-research",
      subjectId: "camera",
      title: "Camera",
      status: "active",
      summary: "",
      createdAt: 1,
      updatedAt: 1,
    });
    await expect(
      runtime.run(
        applyWorkspaceOutputEffect(
          definition,
          output("new-1"),
          { trigger: "message", message: "Update it", subjectId: "camera" },
          logger,
        ),
      ),
    ).rejects.toThrow("Subject-scoped run attempted to update");
  });

  it("validates late references before writing any part of the output", async () =>
    runtime.run(
      Effect.gen(function* () {
        const valid = output();
        const invalid: Output = {
          ...valid,
          sources: [
            ...valid.sources,
            {
              subject_id: "missing-subject",
              title: "Late invalid source",
              url: "https://example.com",
              excerpt: "Must invalidate the complete plan",
            },
          ],
        };

        const exit = yield* Effect.exit(
          applyWorkspaceOutputEffect(
            definition,
            invalid,
            { trigger: "message", message: "Find me a camera" },
            logger,
          ),
        );

        expect(Exit.isFailure(exit)).toBe(true);
        expect(JSON.stringify(exit)).toContain("missing-subject");

        expect(state.subjects.size).toBe(0);
        expect(state.artifacts).toHaveLength(0);
        expect(state.sources).toHaveLength(0);
        expect(state.actions).toHaveLength(0);
        expect(state.messages).toHaveLength(0);
        expect(state.notifications).toHaveLength(0);
      }),
    ));

  it("does not renotify an existing pending action", async () => {
    state.subjects.set("purchase-research:camera", {
      workspaceId: "purchase-research",
      subjectId: "camera",
      title: "Camera",
      status: "active",
      summary: "Camera research",
      createdAt: 1,
      updatedAt: 1,
    });
    state.actions.push({
      actionId: "existing-action",
      workspaceId: "purchase-research",
      subjectId: "camera",
      type: "email_scope",
      status: "pending",
      title: "Watch Sale Emails",
      description: "Watch one retailer.",
      payload: JSON.stringify({
        senders: ["alerts@example.com"],
        domains: [],
        subjectKeywords: [],
        bodyKeywords: [],
      }),
      createdAt: 1,
    });
    await runtime.run(
      applyWorkspaceOutputEffect(
        definition,
        output("camera"),
        { trigger: "message", message: "Check again", subjectId: "camera" },
        logger,
      ),
    );
    expect(state.deliver).not.toHaveBeenCalled();
  });
});

describe("normalizeWorkspaceWebUrl", () => {
  it("allows only HTTP web links", () => {
    expect(normalizeWorkspaceWebUrl("https://example.com/deal")).toBe(
      "https://example.com/deal",
    );
    expect(normalizeWorkspaceWebUrl("http://example.com/deal")).toBe(
      "http://example.com/deal",
    );
    expect(normalizeWorkspaceWebUrl("javascript:alert(1)")).toBeUndefined();
    expect(normalizeWorkspaceWebUrl("data:text/html,bad")).toBeUndefined();
    expect(normalizeWorkspaceWebUrl("not a URL")).toBeUndefined();
  });
});
