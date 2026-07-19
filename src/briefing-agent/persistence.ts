import { Entity } from "@micthiesen/mitools/entities";

export type BriefingNotification = {
  title: string;
  message: string;
  url: string;
  timestamp: number;
  /** Task-run id this notification was produced by, for jumping to its logs. */
  runId?: string;
  /**
   * LLM cost (USD cents) of producing this notification. `null` means the
   * model had no pricing data (see `hasPrice` in `src/ai/cost.ts`);
   * `undefined` means the cost was never computed (older rows).
   */
  costCents?: number | null;
};

export type BriefingHistoryData = {
  briefingName: string;
  notifications: BriefingNotification[];
};

const MAX_NOTIFICATIONS = 50;

export const BriefingHistoryEntity = new Entity<BriefingHistoryData, ["briefingName"]>(
  "briefing-history",
  ["briefingName"],
);

export function getBriefingHistory(briefingName: string): BriefingHistoryData {
  return (
    BriefingHistoryEntity.get({ briefingName }) ?? {
      briefingName,
      notifications: [],
    }
  );
}

export function getAllBriefingHistories(): BriefingHistoryData[] {
  return BriefingHistoryEntity.getAll();
}

export function addBriefingNotification(
  briefingName: string,
  notification: BriefingNotification,
): void {
  const history = getBriefingHistory(briefingName);
  history.notifications.push(notification);
  if (history.notifications.length > MAX_NOTIFICATIONS) {
    history.notifications = history.notifications.slice(-MAX_NOTIFICATIONS);
  }
  BriefingHistoryEntity.upsert(history);
}

/**
 * Backfill a run's total LLM cost across the notifications it produced. Token
 * usage is only fully known after `generateText` resolves (notifications are
 * created earlier, inside the `send_notification` tool), so cost is patched in
 * here. Scoping by `runId` means a run that emitted several notifications
 * splits its cost evenly across them — rather than dumping the whole run's
 * cost onto the last row and leaving the earlier ones uncosted. No-op if the
 * run produced no notifications.
 */
export function distributeBriefingRunCost(
  briefingName: string,
  runId: string | undefined,
  totalCostCents: number | null,
): void {
  const history = getBriefingHistory(briefingName);
  const own = runId
    ? history.notifications.filter((n) => n.runId === runId)
    : history.notifications.slice(-1);
  if (own.length === 0) return;
  const per = totalCostCents === null ? null : totalCostCents / own.length;
  for (const notification of own) notification.costCents = per;
  BriefingHistoryEntity.upsert(history);
}

export function formatNotifications(
  notifications: BriefingNotification[],
  count: number,
): string {
  if (count <= 0 || notifications.length === 0) {
    return "- No previous notifications";
  }

  const recent = notifications.slice(-count);
  return recent
    .map((n) => {
      const date = new Date(n.timestamp).toLocaleDateString("en-US", {
        month: "short",
        day: "numeric",
      });
      const time = new Date(n.timestamp).toLocaleTimeString("en-US", {
        hour: "numeric",
        minute: "2-digit",
      });
      return `- ${n.title} (${n.url}) [${date}, ${time}]`;
    })
    .join("\n");
}

export function formatNotificationHistory(briefingName: string, count: number): string {
  const { notifications } = getBriefingHistory(briefingName);
  return formatNotifications(notifications, count);
}

export function resolveHistoryPlaceholders(
  prompt: string,
  briefingName: string,
): string {
  return prompt.replace(/\{\{history:(\d+)\}\}/g, (_match, digits) => {
    const count = Number.parseInt(digits, 10);
    return formatNotificationHistory(briefingName, count);
  });
}
