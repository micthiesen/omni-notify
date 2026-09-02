import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Clock, Effect, Option } from "effect";

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

type BriefingDeliveryData = {
  briefingName: string;
  deliveryId: string;
  status: "sending" | "delivered";
  updatedAt: number;
};

const MAX_NOTIFICATIONS = 50;

export const BriefingHistoryEntity = new Entity<BriefingHistoryData, ["briefingName"]>(
  "briefing-history",
  ["briefingName"],
);

export const BriefingDeliveryEntity = new Entity<
  BriefingDeliveryData,
  ["briefingName", "deliveryId"]
>("briefing-delivery", ["briefingName", "deliveryId"]);

/** Reserve one model tool call before Pushover so AI retries cannot duplicate it. */
export function reserveBriefingDelivery(briefingName: string, deliveryId: string) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    const key = { briefingName, deliveryId };
    const pk = BriefingDeliveryEntity.getPk(key);
    return yield* docstore.transaction("reserve briefing delivery", (tx) => {
      if (tx.getRawRow(pk, now)) return false;
      tx.upsertDoc(
        pk,
        { ...key, status: "sending", updatedAt: now },
        {
          entity: BriefingDeliveryEntity.name,
        },
        now,
      );
      return true;
    });
  });
}

/** Keep successful delivery reservations permanently as idempotency records. */
export function completeBriefingDelivery(briefingName: string, deliveryId: string) {
  return Effect.gen(function* () {
    yield* BriefingDeliveryEntity.upsert({
      briefingName,
      deliveryId,
      status: "delivered",
      updatedAt: yield* Clock.currentTimeMillis,
    });
  });
}

/** A confirmed provider failure is safe to retry. */
export function releaseBriefingDelivery(briefingName: string, deliveryId: string) {
  return BriefingDeliveryEntity.delete({ briefingName, deliveryId }).pipe(
    Effect.asVoid,
  );
}

export function getBriefingHistory(briefingName: string) {
  return BriefingHistoryEntity.get({ briefingName }).pipe(
    Effect.map(
      Option.getOrElse((): BriefingHistoryData => ({
        briefingName,
        notifications: [],
      })),
    ),
  );
}

export function getAllBriefingHistories() {
  return BriefingHistoryEntity.getAll();
}

export function addBriefingNotification(
  briefingName: string,
  notification: BriefingNotification,
) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    const pk = BriefingHistoryEntity.getPk({ briefingName });
    yield* docstore.transaction("append briefing notification", (tx) => {
      const raw = tx.getRawRow(pk, now);
      const previous = raw
        ? decodeDoc<BriefingHistoryData>(raw.data)
        : { briefingName, notifications: [] };
      const history: BriefingHistoryData = {
        briefingName,
        notifications: [...previous.notifications, notification].slice(
          -MAX_NOTIFICATIONS,
        ),
      };
      tx.upsertDoc(pk, history, { entity: BriefingHistoryEntity.name }, now);
    });
  });
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
) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    const pk = BriefingHistoryEntity.getPk({ briefingName });
    yield* docstore.transaction("distribute briefing run cost", (tx) => {
      const raw = tx.getRawRow(pk, now);
      if (!raw) return;
      const history = decodeDoc<BriefingHistoryData>(raw.data);
      const own = runId
        ? history.notifications.filter((notification) => notification.runId === runId)
        : history.notifications.slice(-1);
      if (own.length === 0) return;
      const per = totalCostCents === null ? null : totalCostCents / own.length;
      const ownSet = new Set(own);
      const updated: BriefingHistoryData = {
        ...history,
        notifications: history.notifications.map((notification) =>
          ownSet.has(notification) ? { ...notification, costCents: per } : notification,
        ),
      };
      tx.upsertDoc(pk, updated, { entity: BriefingHistoryEntity.name }, now);
    });
  });
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

export function formatNotificationHistory(briefingName: string, count: number) {
  return getBriefingHistory(briefingName).pipe(
    Effect.map(({ notifications }) => formatNotifications(notifications, count)),
  );
}

export function resolveHistoryPlaceholders(prompt: string, briefingName: string) {
  const matches = [...prompt.matchAll(/\{\{history:(\d+)\}\}/g)];
  return Effect.gen(function* () {
    let resolved = prompt;
    for (const match of matches) {
      const replacement = yield* formatNotificationHistory(
        briefingName,
        Number.parseInt(match[1], 10),
      );
      resolved = resolved.replace(match[0], replacement);
    }
    return resolved;
  });
}
