import type { Logger } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceActionData } from "./persistence.js";

const mocks = vi.hoisted(() => ({
  action: undefined as WorkspaceActionData | undefined,
  createCalendarEvent: vi.fn(),
  discoverCaldavSession: vi.fn(),
  upsertWorkspaceEmailScope: vi.fn(),
}));

vi.mock("../calendar-events/caldav/index.js", () => ({
  createCalendarEvent: mocks.createCalendarEvent,
  discoverCaldavSession: mocks.discoverCaldavSession,
}));

vi.mock("./persistence.js", () => ({
  getWorkspaceAction: (actionId: string) =>
    mocks.action?.actionId === actionId ? mocks.action : undefined,
  setWorkspaceActionResult: (
    actionId: string,
    status: WorkspaceActionData["status"],
    result: string,
  ) => {
    if (mocks.action?.actionId !== actionId) return undefined;
    mocks.action = { ...mocks.action, status, result, resolvedAt: Date.now() };
    return mocks.action;
  },
  upsertWorkspaceEmailScope: mocks.upsertWorkspaceEmailScope,
}));

import { approveWorkspaceAction, rejectWorkspaceAction } from "./actions.js";

const logger = {} as Logger;

function action(
  type: WorkspaceActionData["type"],
  payload: unknown,
): WorkspaceActionData {
  return {
    actionId: "action-1",
    workspaceId: "purchase-research",
    subjectId: "subject-1",
    type,
    status: "pending",
    title: "Test action",
    description: "Test action description",
    payload: JSON.stringify(payload),
    createdAt: 1,
  };
}

describe("workspace action approval", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.discoverCaldavSession.mockResolvedValue({
      calendarUrl: "https://calendar.example/",
      authHeader: "Basic test",
    });
  });

  it("enables a validated email scope only after approval", async () => {
    mocks.action = action("email_scope", {
      senders: ["orders@example.com"],
      domains: [],
      subjectKeywords: [],
      bodyKeywords: [],
    });

    const approved = await approveWorkspaceAction("action-1", logger);

    expect(approved.status).toBe("approved");
    expect(mocks.upsertWorkspaceEmailScope).toHaveBeenCalledWith(
      "purchase-research",
      "subject-1",
      expect.objectContaining({ senders: ["orders@example.com"] }),
    );
  });

  it("uses a deterministic calendar UID and treats an existing PUT as success", async () => {
    mocks.action = action("calendar_event", {
      title: "Return deadline",
      startDate: "2026-09-01",
      allDay: true,
    });
    mocks.createCalendarEvent.mockResolvedValue({
      status: "error",
      code: 412,
      message: "Precondition failed",
    });

    const approved = await approveWorkspaceAction("action-1", logger);

    expect(approved.status).toBe("approved");
    expect(approved.result).toBe("Calendar event was already created");
    expect(mocks.createCalendarEvent).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ title: "Return deadline" }),
      logger,
      "workspace-action-1@omni-notify",
    );
  });

  it("allows retrying a transiently failed calendar approval", async () => {
    mocks.action = action("calendar_event", {
      title: "Return deadline",
      startDate: "2026-09-01",
      allDay: true,
    });
    mocks.createCalendarEvent
      .mockResolvedValueOnce({ status: "error", code: 503, message: "Unavailable" })
      .mockResolvedValueOnce({ status: "success", eventUid: "workspace-action-1" });

    await expect(approveWorkspaceAction("action-1", logger)).rejects.toThrow(
      "Unavailable",
    );
    expect(mocks.action?.status).toBe("failed");
    await expect(approveWorkspaceAction("action-1", logger)).resolves.toMatchObject({
      status: "approved",
    });
  });

  it("blocks rejection while an approval side effect is in flight", async () => {
    mocks.action = action("calendar_event", {
      title: "Return deadline",
      startDate: "2026-09-01",
      allDay: true,
    });
    let finishCreate:
      | ((value: { status: "success"; eventUid: string }) => void)
      | undefined;
    mocks.createCalendarEvent.mockImplementation(
      () =>
        new Promise((resolve) => {
          finishCreate = resolve;
        }),
    );

    const approval = approveWorkspaceAction("action-1", logger);
    await vi.waitFor(() => expect(mocks.createCalendarEvent).toHaveBeenCalled());
    expect(() => rejectWorkspaceAction("action-1")).toThrow("already being resolved");
    finishCreate?.({ status: "success", eventUid: "workspace-action-1" });
    await expect(approval).resolves.toMatchObject({ status: "approved" });
  });
});
