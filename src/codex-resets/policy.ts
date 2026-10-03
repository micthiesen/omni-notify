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

/** Push previews should contain the news, not a clipped reply thread or raw URLs. */
function compactSummary(text: string): string {
  const clean = text
    .replace(/https?:\/\/\S+/g, "")
    .replace(/\s+/g, " ")
    .trim();
  if (clean.length <= 200) return clean;
  const prefix = clean.slice(0, 197);
  const space = prefix.lastIndexOf(" ");
  return `${prefix.slice(0, space > 150 ? space : 197).trimEnd()}…`;
}

function sourceLabel(sourceUrl: string): string {
  const source = new URL(sourceUrl);
  if (
    ["x.com", "twitter.com", "www.x.com", "www.twitter.com"].includes(source.hostname)
  ) {
    const handle = source.pathname.split("/")[1];
    if (/^[a-zA-Z0-9_]{1,15}$/.test(handle)) return `@${handle} via Reset Beacon`;
  }
  return source.hostname === "resetbeacon.com"
    ? "Reset Beacon"
    : `${source.hostname.slice(0, 80)} via Reset Beacon`;
}

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
      rollout: "rolling out",
      landed: "landed",
      update: "update",
    };
    const scopes: Record<string, string> = {
      all: "all users",
      plus_pro: "Plus / Pro",
      pro: "Pro",
      model: "model-specific",
      unknown: "scope unspecified",
    };
    const scope = event
      ? (scopes[event.scope] ?? "scope unspecified")
      : "scope unspecified";
    const typeLine = `${type === "unspecified" ? "Reset type unspecified" : type === "banked" ? "Banked credit" : "Non-banked reset"}; ${scope}.`;
    const timing =
      item.targetAt && stage === "scheduled"
        ? `Expected ${pacificTime(item.targetAt)}.${Date.parse(item.targetAt) <= now ? " Time has passed; landing is not yet confirmed." : ""}`
        : "";
    const news =
      stage === "landed"
        ? "Reported complete. Check your Usage page."
        : stage === "rollout"
          ? "Observed on the tracker's account; your account may update later."
          : stage === "scheduled"
            ? timing || "A reset has been announced; timing is unspecified."
            : compactSummary(item.summary);
    const guidance =
      type === "banked"
        ? "Saved credit; current usage is unchanged until redeemed."
        : stage === "likely"
          ? "Prediction or hint, not confirmation."
          : stage === "update"
            ? "Completion is not established."
            : stage === "scheduled" &&
                (!item.targetAt || Date.parse(item.targetAt) > now)
              ? "Use remaining allowance beforehand if useful."
              : "";
    const posted = item.sourcePublishedAt
      ? `Posted ${pacificTime(item.sourcePublishedAt)}.`
      : "";
    return [
      {
        // Ignore cosmetic feed revisions, but retain stage, type and schedule changes.
        key: `${item.eventId}:${stage}:${type}${stage === "scheduled" ? `:${item.targetAt ?? "unspecified"}` : ""}`,
        aliases: item.postId
          ? [
              `post:${item.postId}:${stage}:${type}${stage === "scheduled" ? `:${item.targetAt ?? "unspecified"}` : ""}`,
            ]
          : [],
        title: `Codex reset ${labels[stage]}`,
        message: [
          typeLine,
          news,
          guidance,
          posted,
          `Source: ${sourceLabel(item.sourceUrl)}`,
        ]
          .filter(Boolean)
          .join("\n"),
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
