import type { Logger } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it, vi } from "vitest";
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
}));

vi.mock("./persistence.js", () => ({
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
  addWorkspaceSource: (input: Omit<WorkspaceSourceData, "sourceId" | "createdAt">) => {
    const source: WorkspaceSourceData = {
      ...input,
      sourceId: `source-${state.sources.length}`,
      createdAt: 1,
    };
    state.sources.push(source);
    return source;
  },
  getLatestWorkspaceArtifacts: () => [],
  getWorkspaceSubject: (workspaceId: string, subjectId: string) =>
    state.subjects.get(`${workspaceId}:${subjectId}`),
  listWorkspaceEmailScopes: () => [],
  listWorkspaceMessages: () => [],
  listWorkspaceSources: () => [],
  listWorkspaceSubjects: () => [],
  queueWorkspaceNotification: (input: Record<string, unknown>) => {
    const notification = { ...input, status: "pending", attempts: 0 };
    state.notifications.push(notification);
    return { notification, created: true };
  },
  reportWorkspacePapercut: (input: Record<string, unknown>) => {
    state.papercuts.push(input);
    return { ...input, papercutId: "papercut-1", occurrences: 1 };
  },
  upsertWorkspaceSubject: (input: WorkspaceSubjectData) => {
    const subject = { ...input, createdAt: input.createdAt ?? 1, updatedAt: 1 };
    state.subjects.set(`${subject.workspaceId}:${subject.subjectId}`, subject);
    return subject;
  },
}));

import { applyWorkspaceOutput, normalizeWorkspaceWebUrl } from "./engine.js";

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
const logger = { warn: vi.fn() } as unknown as Logger;

type Output = Parameters<typeof applyWorkspaceOutput>[1];

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
    const result = await applyWorkspaceOutput(
      definition,
      output(),
      { trigger: "message", message: "Find me a camera" },
      logger,
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

  it("rejects hallucinated subject IDs instead of allocating a dossier", async () => {
    await expect(
      applyWorkspaceOutput(
        definition,
        output("typo-subject"),
        { trigger: "message", message: "Update it" },
        logger,
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
      applyWorkspaceOutput(
        definition,
        output("new-1"),
        { trigger: "message", message: "Update it", subjectId: "camera" },
        logger,
      ),
    ).rejects.toThrow("Subject-scoped run attempted to update");
  });

  it("does not renotify an existing pending action", async () => {
    state.actionCreated = false;
    await applyWorkspaceOutput(
      definition,
      output(),
      { trigger: "message", message: "Check again" },
      logger,
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
