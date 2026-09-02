import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Effect } from "effect";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import type { FetchedEmail } from "../email/types.js";
import type {
  WorkspaceEmailScopeData,
  WorkspaceSourceData,
  WorkspaceSubjectData,
} from "./persistence.js";
import { WorkspaceOperationError } from "./errors.js";

const mocks = vi.hoisted(() => ({
  sources: new Map<string, WorkspaceSourceData>(),
  subject: undefined as WorkspaceSubjectData | undefined,
  scopes: [] as WorkspaceEmailScopeData[],
}));

vi.mock("./persistence.js", () => ({
  addWorkspaceSource: (source: WorkspaceSourceData) =>
    Effect.sync(() => {
      mocks.sources.set(source.sourceId, source);
      return source;
    }),
  getWorkspaceSource: (sourceId: string) => Effect.succeed(mocks.sources.get(sourceId)),
  getWorkspaceSubject: () => Effect.succeed(mocks.subject),
  listAllWorkspaceEmailScopes: () => Effect.succeed(mocks.scopes),
  markWorkspaceSourcesTriggered: (sourceIds: string[]) =>
    Effect.sync(() => {
      for (const sourceId of sourceIds) {
        const source = mocks.sources.get(sourceId);
        if (source) source.triggeredAt = 1;
      }
    }),
}));

import { WorkspaceEmailHandler, type WorkspaceEmailTrigger } from "./email.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

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
    const trigger = vi.fn(() => Effect.void);
    const handler = new WorkspaceEmailHandler(trigger, {
      info: vi.fn(() => Effect.void),
    } as never);

    await runtime.run(handler.handleEmailsEffect([email]));
    await runtime.run(handler.handleEmailsEffect([email]));

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
    const trigger = vi.fn(() => Effect.void);
    const handler = new WorkspaceEmailHandler(trigger, {
      info: vi.fn(() => Effect.void),
    } as never);

    await runtime.run(handler.handleEmailsEffect([email]));

    expect(mocks.sources.size).toBe(0);
    expect(trigger).not.toHaveBeenCalled();
  });

  it("retries a persisted email whose workspace trigger failed", async () => {
    const trigger = vi.fn<WorkspaceEmailTrigger>();
    trigger.mockReturnValue(Effect.void);
    trigger.mockImplementationOnce(() =>
      Effect.fail(
        new WorkspaceOperationError({
          operation: "workspace run",
          cause: new Error("Workspace run failed"),
        }),
      ),
    );
    const handler = new WorkspaceEmailHandler(trigger, {
      info: vi.fn(() => Effect.void),
    } as never);

    await expect(runtime.run(handler.handleEmailsEffect([email]))).rejects.toThrow(
      "Workspace run failed",
    );
    expect(mocks.sources.size).toBe(1);
    expect([...mocks.sources.values()][0]?.triggeredAt).toBeUndefined();

    await runtime.run(handler.handleEmailsEffect([email]));
    expect(trigger).toHaveBeenCalledTimes(2);
    expect([...mocks.sources.values()][0]?.triggeredAt).toBe(1);
  });

  it("does not mark sources triggered until the workspace run completes", async () => {
    let complete!: () => void;
    const trigger = vi.fn(() =>
      Effect.callback<void>((resume) => {
        complete = () => resume(Effect.void);
      }),
    );
    const handler = new WorkspaceEmailHandler(trigger, {
      info: vi.fn(() => Effect.void),
    } as never);

    const handling = runtime.run(handler.handleEmailsEffect([email]));
    await vi.waitFor(() => expect(trigger).toHaveBeenCalledTimes(1));
    expect([...mocks.sources.values()][0]?.triggeredAt).toBeUndefined();

    complete();
    await handling;
    expect([...mocks.sources.values()][0]?.triggeredAt).toBe(1);
  });
});
