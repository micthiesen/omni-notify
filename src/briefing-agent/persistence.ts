import { Entity } from "@micthiesen/mitools/entities";

export type BriefingNotification = {
  title: string;
  message: string;
  url: string;
  timestamp: number;
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

export function formatNotifications(
  notifications: BriefingNotification[],
  count: number,
): string {
  if (count <= 0 || notifications.length === 0) {
    return "- No previous notifications";
  }

  const recent = notifications.slice(-count);
  return recent.map((n) => `- ${n.title} (${n.url})`).join("\n");
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
