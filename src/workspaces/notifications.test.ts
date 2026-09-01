import type { Logger } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceNotificationData } from "./persistence.js";

const mocks = vi.hoisted(() => ({
  notify: vi.fn(),
  markFailed: vi.fn(),
  markSending: vi.fn(),
  markSent: vi.fn(),
  markUnknown: vi.fn(),
}));

vi.mock("@micthiesen/mitools/pushover", () => ({ notify: mocks.notify }));
vi.mock("./persistence.js", () => ({
  listDueWorkspaceNotifications: vi.fn().mockReturnValue([]),
  markWorkspaceNotificationFailed: mocks.markFailed,
  markWorkspaceNotificationSending: mocks.markSending,
  markWorkspaceNotificationSent: mocks.markSent,
  markWorkspaceNotificationUnknown: mocks.markUnknown,
}));

import { deliverWorkspaceNotification } from "./notifications.js";

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
const logger = { warn: vi.fn() } as unknown as Logger;

describe("deliverWorkspaceNotification", () => {
  beforeEach(() => vi.clearAllMocks());

  it("records success after Pushover accepts the notification", async () => {
    mocks.notify.mockResolvedValue(undefined);
    await expect(deliverWorkspaceNotification(notification, logger)).resolves.toBe(
      true,
    );
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

    await expect(deliverWorkspaceNotification(notification, logger)).rejects.toThrow(
      "database unavailable",
    );
    expect(mocks.notify).toHaveBeenCalledTimes(1);

    await expect(
      deliverWorkspaceNotification(
        { ...notification, status: "sending", attempts: 1 },
        logger,
      ),
    ).resolves.toBe(true);
    expect(mocks.notify).toHaveBeenCalledTimes(1);
    expect(mocks.markSent).toHaveBeenCalledTimes(1);
    expect(mocks.markUnknown).toHaveBeenCalledWith("notification-1");
  });

  it("keeps a failed delivery queued with its attempt count", async () => {
    mocks.notify.mockRejectedValue(new Error("Pushover unavailable"));
    await expect(deliverWorkspaceNotification(notification, logger)).resolves.toBe(
      false,
    );
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
    const first = deliverWorkspaceNotification(notification, logger);
    await vi.waitFor(() => expect(mocks.notify).toHaveBeenCalledTimes(1));
    await expect(deliverWorkspaceNotification(notification, logger)).resolves.toBe(
      false,
    );
    finish?.();
    await expect(first).resolves.toBe(true);
    expect(mocks.notify).toHaveBeenCalledTimes(1);
  });
});
