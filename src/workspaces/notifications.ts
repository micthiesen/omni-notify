import type { NamedLogger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import config from "../utils/config.js";
import { WorkspaceOperationError } from "./errors.js";
import {
  listDueWorkspaceNotifications,
  markWorkspaceNotificationFailed,
  markWorkspaceNotificationSending,
  markWorkspaceNotificationSent,
  markWorkspaceNotificationUnknown,
  type WorkspaceNotificationData,
} from "./persistence.js";
import { workspaceRepositoryEffect } from "./repository.js";
import type { TaskServices } from "../task-runs/registry.js";

const deliveringNotifications = new Set<string>();

export function deliverWorkspaceNotificationEffect(
  notification: WorkspaceNotificationData,
  logger: NamedLogger,
) {
  if (notification.status === "sent" || notification.status === "unknown") {
    return Effect.succeed(true);
  }
  const acquire = Effect.sync(() => {
    if (deliveringNotifications.has(notification.notificationId)) return false;
    deliveringNotifications.add(notification.notificationId);
    return true;
  });
  return Effect.acquireUseRelease(
    acquire,
    (acquired) => {
      if (!acquired) return Effect.succeed(false);
      if (notification.status === "sending") {
        return logger
          .warn(
            `Workspace notification ${notification.notificationId} had an unacknowledged provider attempt; acknowledging without resending`,
          )
          .pipe(
            Effect.andThen(
              workspaceRepositoryEffect(
                "acknowledge reserved workspace notification",
                () => markWorkspaceNotificationUnknown(notification.notificationId),
              ),
            ),
            Effect.as(true),
          );
      }
      const attempts = notification.attempts + 1;
      return workspaceRepositoryEffect("reserve workspace notification delivery", () =>
        markWorkspaceNotificationSending(notification.notificationId, attempts),
      ).pipe(
        Effect.andThen(
          Effect.result(
            notify({
              title: notification.title,
              message: notification.message,
              url: notification.url,
              url_title: notification.urlTitle,
              token: config.PUSHOVER_WORKSPACE_TOKEN,
            }).pipe(
              Effect.mapError(
                (cause) =>
                  new WorkspaceOperationError({
                    operation: "send workspace notification",
                    cause,
                  }),
              ),
            ),
          ),
        ),
        Effect.flatMap((delivery) => {
          if (delivery._tag === "Success") {
            return workspaceRepositoryEffect("mark workspace notification sent", () =>
              markWorkspaceNotificationSent(notification.notificationId),
            ).pipe(Effect.as(true));
          }
          const message = delivery.failure.message;
          return workspaceRepositoryEffect(
            "record workspace notification provider failure",
            () =>
              markWorkspaceNotificationFailed(
                notification.notificationId,
                attempts,
                message,
              ),
          ).pipe(
            Effect.tap(() =>
              logger.warn(
                `Workspace notification ${notification.notificationId} failed (attempt ${attempts}); queued for retry`,
                message,
              ),
            ),
            Effect.as(false),
          );
        }),
      );
    },
    (acquired) =>
      Effect.sync(() => {
        if (acquired) deliveringNotifications.delete(notification.notificationId);
      }),
  );
}

export class WorkspaceNotificationTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "WorkspaceNotifications";
  public readonly displayName = "Workspace Notifications";
  public readonly schedule = "*/5 * * * *";
  private readonly logger: NamedLogger;
  private lastSummary?: string;

  public constructor(parentLogger: NamedLogger) {
    this.logger = parentLogger.extend("WorkspaceNotificationTask");
  }

  public readonly run = Effect.gen({ self: this }, function* () {
    const due = yield* workspaceRepositoryEffect(
      "list due workspace notifications",
      () => listDueWorkspaceNotifications(),
    );
    const delivered = yield* Effect.forEach(
      due,
      (notification) => deliverWorkspaceNotificationEffect(notification, this.logger),
      { concurrency: 4 },
    );
    const sent = delivered.filter(Boolean).length;
    this.lastSummary = `Sent ${sent} notification(s); ${due.length - sent} queued for retry`;
  });

  public getLastRunSummary(): string | undefined {
    return this.lastSummary;
  }
}
