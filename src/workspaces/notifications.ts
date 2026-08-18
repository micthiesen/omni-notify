import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import config from "../utils/config.js";
import {
  listDueWorkspaceNotifications,
  markWorkspaceNotificationFailed,
  markWorkspaceNotificationSent,
  type WorkspaceNotificationData,
} from "./persistence.js";

const deliveringNotifications = new Set<string>();

export async function deliverWorkspaceNotification(
  notification: WorkspaceNotificationData,
  logger: Logger,
): Promise<boolean> {
  if (notification.status === "sent") return true;
  if (deliveringNotifications.has(notification.notificationId)) return false;
  deliveringNotifications.add(notification.notificationId);
  try {
    await notify({
      title: notification.title,
      message: notification.message,
      url: notification.url,
      url_title: notification.urlTitle,
      token: config.PUSHOVER_WORKSPACE_TOKEN,
    });
    markWorkspaceNotificationSent(notification.notificationId);
    return true;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const attempts = notification.attempts + 1;
    markWorkspaceNotificationFailed(notification.notificationId, attempts, message);
    logger.warn(
      `Workspace notification ${notification.notificationId} failed (attempt ${attempts}); queued for retry`,
      message,
    );
    return false;
  } finally {
    deliveringNotifications.delete(notification.notificationId);
  }
}

export class WorkspaceNotificationTask extends ScheduledTask {
  public readonly name = "WorkspaceNotifications";
  public readonly displayName = "Workspace Notifications";
  public readonly schedule = "*/5 * * * *";
  private readonly logger: Logger;
  private lastSummary?: string;

  public constructor(parentLogger: Logger) {
    super();
    this.logger = parentLogger.extend("WorkspaceNotificationTask");
  }

  public async run(): Promise<void> {
    const due = listDueWorkspaceNotifications();
    let sent = 0;
    for (const notification of due) {
      if (await deliverWorkspaceNotification(notification, this.logger)) sent += 1;
    }
    this.lastSummary = `Sent ${sent} notification(s); ${due.length - sent} queued for retry`;
  }

  public getLastRunSummary(): string | undefined {
    return this.lastSummary;
  }
}
