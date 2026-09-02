import type { Logger } from "@micthiesen/mitools/logging";
import { it as effectIt } from "@effect/vitest";
import { Effect, Fiber } from "effect";
import { beforeEach, describe, expect, vi } from "vitest";
import type { WorkspaceActionData } from "./persistence.js";

const mocks = vi.hoisted(() => ({
  action: undefined as WorkspaceActionData | undefined,
  createCalendarEvent: vi.fn(),
  discoverCaldavSession: vi.fn(),
  upsertWorkspaceEmailScope: vi.fn(),
}));

vi.mock("../calendar-events/caldav/index.js", () => ({
  createCalendarEventEffect: (...args: unknown[]) =>
    Effect.promise(() => mocks.createCalendarEvent(...args)),
  discoverCaldavSessionEffect: (...args: unknown[]) =>
    Effect.promise(() => mocks.discoverCaldavSession(...args)),
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

import {
  approveWorkspaceActionEffect,
  rejectWorkspaceActionEffect,
} from "./actions.js";

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

  effectIt.effect("enables a validated email scope only after approval", () =>
    Effect.gen(function* () {
      mocks.action = action("email_scope", {
        senders: ["orders@example.com"],
        domains: [],
        subjectKeywords: [],
        bodyKeywords: [],
      });

      const approved = yield* approveWorkspaceActionEffect("action-1", logger);

      expect(approved.status).toBe("approved");
      expect(mocks.upsertWorkspaceEmailScope).toHaveBeenCalledWith(
        "purchase-research",
        "subject-1",
        expect.objectContaining({ senders: ["orders@example.com"] }),
      );
    }),
  );

  effectIt.effect(
    "uses a deterministic calendar UID and treats an existing PUT as success",
    () =>
      Effect.gen(function* () {
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

        const approved = yield* approveWorkspaceActionEffect("action-1", logger);

        expect(approved.status).toBe("approved");
        expect(approved.result).toBe("Calendar event was already created");
        expect(mocks.createCalendarEvent).toHaveBeenCalledWith(
          expect.anything(),
          expect.objectContaining({ title: "Return deadline" }),
          logger,
          "workspace-action-1@omni-notify",
        );
      }),
  );

  effectIt.effect("allows retrying a transiently failed calendar approval", () =>
    Effect.gen(function* () {
      mocks.action = action("calendar_event", {
        title: "Return deadline",
        startDate: "2026-09-01",
        allDay: true,
      });
      mocks.createCalendarEvent
        .mockResolvedValueOnce({ status: "error", code: 503, message: "Unavailable" })
        .mockResolvedValueOnce({ status: "success", eventUid: "workspace-action-1" });

      const failure = yield* Effect.flip(
        approveWorkspaceActionEffect("action-1", logger),
      );
      expect(failure.message).toContain("Unavailable");
      expect(mocks.action?.status).toBe("failed");
      const approved = yield* approveWorkspaceActionEffect("action-1", logger);
      expect(approved).toMatchObject({ status: "approved" });
    }),
  );

  effectIt.effect("blocks rejection while an approval side effect is in flight", () =>
    Effect.gen(function* () {
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

      const approval = yield* Effect.forkChild(
        approveWorkspaceActionEffect("action-1", logger),
      );
      yield* Effect.promise(() =>
        vi.waitFor(() => expect(mocks.createCalendarEvent).toHaveBeenCalled()),
      );
      const rejection = yield* Effect.flip(rejectWorkspaceActionEffect("action-1"));
      expect(rejection.message).toContain("already being resolved");
      finishCreate?.({ status: "success", eventUid: "workspace-action-1" });
      expect(yield* Fiber.join(approval)).toMatchObject({ status: "approved" });
    }),
  );
});
