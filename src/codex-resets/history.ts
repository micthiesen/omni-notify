import type { AlertFeed, ResetHistory } from "./source.js";

const LOOKBACK_MS = 48 * 60 * 60_000;
const CLOCK_SKEW_MS = 5 * 60_000;

/**
 * Use durable history to report a completed reset while the alert feed is
 * delayed. Feed entries remain authoritative when they already cover a post.
 */
export function addCompletedHistoryAlerts(
  feed: AlertFeed,
  history: ResetHistory,
  now: number,
): AlertFeed {
  const items = [...feed.items];
  for (const event of history.items) {
    if (
      event.eventKind !== "completed" ||
      event.status !== "completed" ||
      event.kind !== "special_global" ||
      event.scope !== "all" ||
      event.supersededBy ||
      !event.announcedAt
    )
      continue;
    const announcedAt = Date.parse(event.announcedAt);
    if (
      !Number.isFinite(announcedAt) ||
      announcedAt < now - LOOKBACK_MS ||
      announcedAt > now + CLOCK_SKEW_MS
    )
      continue;
    const announcementIds = new Set(
      event.sources.map((source) => source.announcementId),
    );
    const coveredByFeed = items.some(
      (item) =>
        item.postId !== null &&
        announcementIds.has(item.postId) &&
        (item.withdrawn || item.topic === "action" || item.topic === "rollout"),
    );
    const source = event.sources[0];
    if (!source || coveredByFeed) continue;

    const parent = history.items.find(
      (candidate) => candidate.fulfilledBy === event.id,
    );
    if (
      !parent ||
      parent.kind !== event.kind ||
      !["completed", "fulfilled"].includes(parent.status) ||
      !["scheduled", "intent"].includes(parent.eventKind) ||
      parent.scope !== "all"
    )
      continue;
    const parentFeedItem = items.find((item) =>
      parent.sources.some(
        (parentSource) => item.postId === parentSource.announcementId,
      ),
    );
    // The old delivery ledger only knows feed event IDs. If the parent has
    // disappeared, a synthetic ID could replay a pre-upgrade notification.
    if (!parentFeedItem) continue;
    items.push({
      id: `history:${event.id}`,
      eventId: parentFeedItem.eventId,
      postId: source.announcementId,
      topic: "action",
      state: "action_claimed",
      title: "Codex allowance reset completed",
      summary:
        event.summary ??
        "Reset Beacon reports that the Codex allowance reset completed.",
      sourceUrl: source.url,
      evidenceId: null,
      targetAt: null,
      publishedAt: event.announcedAt,
      sourcePublishedAt: event.announcedAt,
      withdrawn: false,
    });
  }
  return { ...feed, items };
}
