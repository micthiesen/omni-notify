import type { ResetAlert } from "./delivery.js";
import type { AlertFeed, ResetHistory } from "./source.js";

export const ALERT_LOOKBACK_MS = 48 * 60 * 60_000;
export const FEED_MAX_AGE_MS = 45 * 60_000;
const CLOCK_SKEW_MS = 5 * 60_000;

export function isFreshFeed(feed: AlertFeed, now: number): boolean {
  const age = now - Date.parse(feed.generatedAt);
  return age >= -CLOCK_SKEW_MS && age <= FEED_MAX_AGE_MS;
}

type Stage = "likely" | "scheduled" | "rollout" | "landed" | "update";
function stageOf(
  item: AlertFeed["items"][number],
  event?: ResetHistory["items"][number],
): Stage | undefined {
  if (
    item.topic === "likely" &&
    ["likely_forecast", "likely_official_intent"].includes(item.state)
  )
    return "likely";
  if (item.topic === "schedule" && item.state === "official_scheduled")
    return "scheduled";
  if (item.topic === "rollout" && item.state === "rollout_observed") return "rollout";
  if (item.topic === "action" && item.state === "action_claimed") {
    // Banked promises and receipts both appear as action_claimed/policy_change.
    // Require completion or measured receipt evidence before claiming a landing.
    return (event?.eventKind === "completed" && event.status === "completed") ||
      (event?.kind === "banked" && event.evidenceClass === "measured_account")
      ? "landed"
      : "update";
  }
  return undefined;
}

const pacificTime = (value: string) =>
  new Intl.DateTimeFormat("en-CA", {
    timeZone: "America/Vancouver",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    timeZoneName: "short",
  }).format(new Date(value));

/** The feed supplies classification; history supplies type and reconciles old hints.
 * An elapsed announcement deadline never constitutes evidence of a landed reset. */
export function selectResetAlerts(
  feed: AlertFeed,
  history: ResetHistory,
  now: number,
): ResetAlert[] {
  if (!isFreshFeed(feed, now)) return [];
  const byPost = new Map(
    history.items.flatMap((event) =>
      event.sources.map((source) => [source.announcementId, event] as const),
    ),
  );
  const liveItems = feed.items.filter((item) => !item.withdrawn);
  const eventFor = (item: AlertFeed["items"][number]) =>
    item.postId ? byPost.get(item.postId) : undefined;
  const candidates = liveItems.flatMap((item): ResetAlert[] => {
    const event = eventFor(item);
    const stage = stageOf(item, event);
    const occurredAt = Date.parse(item.publishedAt);
    if (
      !stage ||
      now - occurredAt > ALERT_LOOKBACK_MS ||
      occurredAt > now + CLOCK_SKEW_MS
    )
      return [];
    if (
      event?.supersededBy ||
      ["superseded", "expired", "missed"].includes(event?.status ?? "")
    )
      return [];
    if (stage === "likely" || stage === "scheduled") {
      if (
        event?.fulfilledBy ||
        ["fulfilled", "completed"].includes(event?.status ?? "")
      )
        return [];
      if (
        liveItems.some(
          (other) =>
            other.eventId === item.eventId &&
            ["landed", "rollout", "update"].includes(
              stageOf(other, eventFor(other)) ?? "",
            ),
        )
      )
        return [];
      if (
        stage === "likely" &&
        liveItems.some(
          (other) => other.eventId === item.eventId && stageOf(other) === "scheduled",
        )
      )
        return [];
      if (stage === "likely" && item.targetAt && Date.parse(item.targetAt) <= now)
        return [];
    }
    const type =
      event?.kind === "banked"
        ? "banked"
        : event?.kind === "special_global"
          ? "non-banked"
          : "unspecified";
    const labels = {
      likely: "looks likely",
      scheduled: "announced",
      rollout: "landing (observed)",
      landed: "reported landed",
      update: "update",
    };
    const timing = item.targetAt
      ? `Announced time: ${pacificTime(item.targetAt)}.${Date.parse(item.targetAt) <= now && stage === "scheduled" ? " Time has passed; landing is not yet confirmed." : ""}`
      : "";
    const action =
      stage === "update"
        ? "Read the source for the current status; completion is not established."
        : type === "banked"
          ? "Saved credit; current usage is unchanged until redeemed."
          : stage === "likely" || stage === "scheduled"
            ? "Use remaining allowance beforehand if useful."
            : "Check your Usage page; account rollout can vary.";
    const caveat = stage === "likely" ? "Prediction or hint, not confirmation." : "";
    const typeLine = `Type: ${type}${event ? `; scope: ${event.scope}` : " (source has not specified)"}.`;
    const evidence =
      item.evidenceId && /^[a-zA-Z0-9-]{1,64}$/.test(item.evidenceId)
        ? `\nArchive: https://resetbeacon.com/evidence/${item.evidenceId}/`
        : "";
    const footer = `\nSource: ${item.sourceUrl}\nTracker: https://resetbeacon.com/${evidence}`;
    const prefix = [typeLine, timing, caveat, action]
      .filter(Boolean)
      .join("\n")
      .slice(0, 300);
    const room = Math.max(0, 1_024 - prefix.length - footer.length - 2);
    const summary =
      item.summary.length > room
        ? `${item.summary.slice(0, Math.max(0, room - 1))}…`
        : item.summary;
    return [
      {
        // Ignore cosmetic feed revisions, but retain stage, type and schedule changes.
        key: `${item.eventId}:${stage}:${type}${stage === "scheduled" ? `:${item.targetAt ?? "unspecified"}` : ""}`,
        title: `Codex reset ${labels[stage]} (${type})`,
        message: `${prefix}\n\n${summary}${footer}`,
        url: item.sourceUrl,
        occurredAt,
      },
    ];
  });
  // A newly enrolled monitor sends current news, not every revision of that news.
  return [
    ...new Map(
      candidates
        .sort((a, b) => a.occurredAt - b.occurredAt)
        .map((item) => [item.key, item]),
    ).values(),
  ];
}
