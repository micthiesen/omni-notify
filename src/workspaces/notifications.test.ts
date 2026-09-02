import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import type { WorkspaceNotificationData } from "./persistence.js";

const mocks = vi.hoisted(() => ({
  notify: vi.fn(),
  markFailed: vi.fn(),
  markSending: vi.fn(),
  markSent: vi.fn(),
  markUnknown: vi.fn(),
}));

vi.mock("@micthiesen/mitools/pushover", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@micthiesen/mitools/pushover")>()),
  notify: (...args: unknown[]) =>
    Effect.tryPromise({
      try: () => mocks.notify(...args),
      catch: (cause) => cause,
    }),
}));
vi.mock("./persistence.js", () => ({
  listDueWorkspaceNotifications: vi.fn(() => Effect.succeed([])),
  markWorkspaceNotificationFailed: (...args: unknown[]) =>
    Effect.sync(() => mocks.markFailed(...args)),
  markWorkspaceNotificationSending: (...args: unknown[]) =>
    Effect.sync(() => mocks.markSending(...args)),
  markWorkspaceNotificationSent: (...args: unknown[]) =>
    Effect.try({
      try: () => mocks.markSent(...args),
      catch: (cause) => cause,
    }),
  markWorkspaceNotificationUnknown: (...args: unknown[]) =>
    Effect.sync(() => mocks.markUnknown(...args)),
}));

import { deliverWorkspaceNotificationEffect } from "./notifications.js";

const notification: WorkspaceNotificationData = {
  notificationId: "notification-1",
  workspaceId: "purchase-research",
  subjectId: "camera",
  title: "Approval Needed",
  message: "Review the scope",
  url: "http://omni.boris/workspaces/purchase-research/camera?target=action-1",
  urlTitle: "Review Action",
  status: "pending",
  attempts: 0,
  createdAt: 1,
  nextAttemptAt: 1,
};
const logger = { warn: vi.fn(() => Effect.void) } as unknown as NamedLogger;
const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

describe("deliverWorkspaceNotification", () => {
  beforeEach(() => vi.clearAllMocks());

  it("records success after Pushover accepts the notification", async () => {
    mocks.notify.mockResolvedValue(undefined);
    await expect(
      runtime.run(deliverWorkspaceNotificationEffect(notification, logger)),
    ).resolves.toBe(true);
    expect(mocks.markSent).toHaveBeenCalledWith("notification-1");
    expect(mocks.markSending).toHaveBeenCalledWith("notification-1", 1);
    expect(mocks.markSending.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.notify.mock.invocationCallOrder[0]!,
    );
  });

  it("does not resend after delivery succeeded but the sent acknowledgement failed", async () => {
    mocks.notify.mockResolvedValue(undefined);
    mocks.markSent.mockImplementationOnce(() => {
      throw new Error("database unavailable");
    });

    await expect(
      runtime.run(deliverWorkspaceNotificationEffect(notification, logger)),
    ).rejects.toThrow("database unavailable");
    expect(mocks.notify).toHaveBeenCalledTimes(1);

    await expect(
      runtime.run(
        deliverWorkspaceNotificationEffect(
          { ...notification, status: "sending", attempts: 1 },
          logger,
        ),
      ),
    ).resolves.toBe(true);
    expect(mocks.notify).toHaveBeenCalledTimes(1);
    expect(mocks.markSent).toHaveBeenCalledTimes(1);
    expect(mocks.markUnknown).toHaveBeenCalledWith("notification-1");
  });

  it("keeps a failed delivery queued with its attempt count", async () => {
    mocks.notify.mockRejectedValue(new Error("Pushover unavailable"));
    await expect(
      runtime.run(deliverWorkspaceNotificationEffect(notification, logger)),
    ).resolves.toBe(false);
    expect(mocks.markFailed).toHaveBeenCalledWith(
      "notification-1",
      1,
      "send workspace notification failed: Pushover unavailable",
    );
  });

  it("does not send the same outbox row concurrently", async () => {
    let finish: (() => void) | undefined;
    mocks.notify.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    const first = runtime.run(deliverWorkspaceNotificationEffect(notification, logger));
    await vi.waitFor(() => expect(mocks.notify).toHaveBeenCalledTimes(1));
    await expect(
      runtime.run(deliverWorkspaceNotificationEffect(notification, logger)),
    ).resolves.toBe(false);
    finish?.();
    await expect(first).resolves.toBe(true);
    expect(mocks.notify).toHaveBeenCalledTimes(1);
  });
});
