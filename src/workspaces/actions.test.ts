import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect, Fiber } from "effect";
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
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
    Effect.succeed(mocks.action?.actionId === actionId ? mocks.action : undefined),
  setWorkspaceActionResult: (
    actionId: string,
    status: WorkspaceActionData["status"],
    result: string,
  ) =>
    Effect.sync(() => {
      if (mocks.action?.actionId !== actionId) return undefined;
      mocks.action = { ...mocks.action, status, result, resolvedAt: Date.now() };
      return mocks.action;
    }),
  upsertWorkspaceEmailScope: (...args: unknown[]) =>
    Effect.sync(() => mocks.upsertWorkspaceEmailScope(...args)),
}));

import {
  approveWorkspaceActionEffect,
  rejectWorkspaceActionEffect,
} from "./actions.js";

const logger = {
  debug: vi.fn(() => Effect.void),
  info: vi.fn(() => Effect.void),
  warn: vi.fn(() => Effect.void),
  error: vi.fn(() => Effect.void),
} as unknown as NamedLogger;
const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

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

  it("enables a validated email scope only after approval", async () =>
    runtime.run(
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
    ));

  it("uses a deterministic calendar UID and treats an existing PUT as success", async () =>
    runtime.run(
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
    ));

  it("allows retrying a transiently failed calendar approval", async () =>
    runtime.run(
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
    ));

  it("blocks rejection while an approval side effect is in flight", async () =>
    runtime.run(
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
    ));
});
