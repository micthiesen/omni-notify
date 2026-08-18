import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FetchedEmail } from "../email/types.js";
import type {
  WorkspaceEmailScopeData,
  WorkspaceSourceData,
  WorkspaceSubjectData,
} from "./persistence.js";

const mocks = vi.hoisted(() => ({
  sources: new Map<string, WorkspaceSourceData>(),
  subject: undefined as WorkspaceSubjectData | undefined,
  scopes: [] as WorkspaceEmailScopeData[],
}));

vi.mock("./persistence.js", () => ({
  addWorkspaceSource: (source: WorkspaceSourceData) => {
    mocks.sources.set(source.sourceId, source);
    return source;
  },
  getWorkspaceSource: (sourceId: string) => mocks.sources.get(sourceId),
  getWorkspaceSubject: () => mocks.subject,
  listAllWorkspaceEmailScopes: () => mocks.scopes,
}));

import { WorkspaceEmailHandler } from "./email.js";

const email: FetchedEmail = {
  id: "email-1",
  subject: "Camera price drop",
  from: "alerts@shop.example",
  textBody: "The camera is now on sale.",
  links: [],
  receivedAt: "2026-08-18T10:00:00Z",
  attachments: [],
};

describe("WorkspaceEmailHandler", () => {
  beforeEach(() => {
    mocks.sources.clear();
    mocks.subject = {
      workspaceId: "purchase-research",
      subjectId: "camera",
      title: "Camera",
      status: "active",
      summary: "",
      createdAt: 1,
      updatedAt: 1,
    };
    mocks.scopes = [
      {
        workspaceId: "purchase-research",
        subjectId: "camera",
        senders: ["alerts@shop.example"],
        domains: [],
        subjectKeywords: [],
        bodyKeywords: [],
        updatedAt: 1,
      },
    ];
  });

  it("persists and triggers a newly matched active-subject email once", async () => {
    const trigger = vi.fn();
    const handler = new WorkspaceEmailHandler(trigger, {
      info: vi.fn(),
    } as never);

    await handler.handleEmails([email]);
    await handler.handleEmails([email]);

    expect(mocks.sources.size).toBe(1);
    expect(trigger).toHaveBeenCalledTimes(1);
    expect(trigger).toHaveBeenCalledWith(
      "purchase-research",
      "camera",
      expect.stringContaining("Camera price drop"),
      "email",
    );
  });

  it("does not ingest for a paused subject", async () => {
    if (mocks.subject) mocks.subject.status = "paused";
    const trigger = vi.fn();
    const handler = new WorkspaceEmailHandler(trigger, {
      info: vi.fn(),
    } as never);

    await handler.handleEmails([email]);

    expect(mocks.sources.size).toBe(0);
    expect(trigger).not.toHaveBeenCalled();
  });
});
