import {
  ALERT_LOOKBACK_MS,
  CLOCK_SKEW_MS,
  pacificTime,
  compactSummary,
} from "../reset-alerts/presentation.js";
import type { ResetAlert } from "../reset-alerts/delivery.js";
import type { ClaudeResetSource } from "./source.js";

function postIdentity(value: string): string {
  const url = new URL(value);
  if (["x.com", "twitter.com", "www.x.com", "www.twitter.com"].includes(url.hostname)) {
    const post = /^\/(?:[^/]+\/status|i\/web\/status)\/(\d+)(?:\/|$)/.exec(
      url.pathname,
    );
    if (post) return `x:${post[1]}`;
  }
  return url.href;
}

/** The catalog update date is editorial, not a polling heartbeat. Its event dates
 * bound replay; revisions never make old announcements new. Reset Radar supplies
 * no structured banked/regular distinction, so retain its wording as a report. */
export function selectClaudeResetAlerts(
  source: ClaudeResetSource,
  now: number,
): ResetAlert[] {
  const candidates = source.events.flatMap((event): ResetAlert[] => {
    const occurredAt = Date.parse(event.date);
    if (
      event.type !== "counter-reset" ||
      event.status !== "historic" ||
      event.confidence !== "confirmed" ||
      !event.surfaces.includes("claude-code") ||
      now - occurredAt > ALERT_LOOKBACK_MS ||
      occurredAt > now + CLOCK_SKEW_MS
    )
      return [];

    const primary = event.sources[0]?.url;
    return [
      {
        key: `${event.id}:reported`,
        // Additional sources can be background references shared by unrelated events.
        aliases: primary ? [`post:${postIdentity(primary)}:reported`] : [],
        title: "Claude Code reset reported",
        message: [
          compactSummary(event.title),
          compactSummary(event.summary),
          "Check Settings → Usage. Banked resets refill usage only when redeemed.",
          "Your account has not been verified.",
          `Reported ${pacificTime(event.date)}.`,
          "Source: Reset Radar",
        ]
          .filter(Boolean)
          .join("\n"),
        url: primary ?? `https://resetradar.com/#${encodeURIComponent(event.id)}`,
        occurredAt,
      },
    ];
  });

  const seen = new Set<string>();
  return candidates
    .sort((a, b) => b.occurredAt - a.occurredAt)
    .filter((alert) => {
      const identities = [alert.key, ...(alert.aliases ?? [])];
      const duplicate = identities.some((identity) => seen.has(identity));
      for (const identity of identities) seen.add(identity);
      return !duplicate;
    })
    .reverse();
}
