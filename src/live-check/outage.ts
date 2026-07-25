import { formatElapsed } from "../utils/dates.js";

/**
 * Consecutive all-unknown ticks before a streamer counts as unreachable. The
 * live check runs every 20s, so this is roughly a minute of failures.
 */
export const UNREACHABLE_TICK_THRESHOLD = 3;

/**
 * Consecutive clean ticks before an outage is declared over. Recovery is a
 * notification too, so a streamer flapping around the threshold must not be
 * able to pump out degraded/recovered pairs every couple of minutes.
 */
export const RECOVERY_TICK_THRESHOLD = 3;

/**
 * Delay before the Nth degraded alert (index = alerts already sent, minus the
 * first which fires immediately). One alert when the outage is confirmed, then
 * increasingly rare reminders while it persists — never one per tick.
 */
const ESCALATION_MS = [30 * 60_000, 2 * 60 * 60_000, 6 * 60 * 60_000, 24 * 60 * 60_000];

/** Names listed inline before the message switches to a "+N more" tail. */
const MAX_NAMES = 5;

/** Distinct error strings quoted in the message. */
const MAX_ERRORS = 2;

export interface UnknownStreak {
  displayName: string;
  /** Consecutive all-unknown ticks so far. */
  ticks: number;
  /** Summary of the errors from the streamer's bindings. */
  error: string;
}

export type OutageAlert =
  | { kind: "degraded"; title: string; message: string }
  | { kind: "recovered"; title: string; message: string };

interface Episode {
  startedAt: number;
  lastAlertAt: number;
  alerts: number;
  clearTicks: number;
}

/**
 * Turns per-streamer unreachability into one outage-level alert stream.
 *
 * Every streamer failing every tick used to mean a notification per streamer
 * per tick; a platform or network blip therefore read as dozens of near
 * identical errors. This collapses that into: one alert when the outage is
 * confirmed, escalating reminders while it lasts, and one recovery note.
 *
 * The caller sends these directly rather than through the generic alert
 * throttle: cadence is decided here, and a second layer of deduplication would
 * only be able to swallow alerts this one deliberately chose to send.
 */
export class OutageAlerter {
  private episode: Episode | undefined;

  /**
   * Evaluate the current tick. `streaks` should list every streamer whose
   * bindings all returned unknown; only those at or above the tick threshold
   * count towards an outage.
   */
  public evaluate(
    streaks: UnknownStreak[],
    totalStreamers: number,
    now: number,
  ): OutageAlert | null {
    const confirmed = streaks.filter((s) => s.ticks >= UNREACHABLE_TICK_THRESHOLD);
    const episode = this.episode;

    if (confirmed.length === 0) {
      if (!episode) return null;
      episode.clearTicks++;
      if (episode.clearTicks < RECOVERY_TICK_THRESHOLD) return null;
      this.episode = undefined;
      return {
        kind: "recovered",
        title: "Live check recovered",
        message: `All streamers reachable again after ${formatElapsed(now - episode.startedAt)}.`,
      };
    }

    if (!episode) {
      this.episode = { startedAt: now, lastAlertAt: now, alerts: 1, clearTicks: 0 };
      return buildDegradedAlert(confirmed, totalStreamers, 0);
    }

    episode.clearTicks = 0;
    const waitMs =
      ESCALATION_MS[Math.min(episode.alerts - 1, ESCALATION_MS.length - 1)];
    if (now - episode.lastAlertAt < waitMs) return null;

    episode.lastAlertAt = now;
    episode.alerts++;
    return buildDegradedAlert(confirmed, totalStreamers, now - episode.startedAt);
  }
}

function buildDegradedAlert(
  confirmed: UnknownStreak[],
  totalStreamers: number,
  elapsedMs: number,
): OutageAlert {
  const sorted = [...confirmed].sort((a, b) => b.ticks - a.ticks);
  const names = sorted.slice(0, MAX_NAMES).map((s) => s.displayName);
  const extraNames = sorted.length - names.length;
  const lines = [
    extraNames > 0 ? `${names.join(", ")} +${extraNames} more` : names.join(", "),
  ];
  if (elapsedMs > 0) lines.push(`Unreachable for ${formatElapsed(elapsedMs)}.`);

  const distinct = [...new Set(sorted.map((s) => s.error).filter(Boolean))];
  const errors = distinct.slice(0, MAX_ERRORS);
  if (errors.length > 0) lines.push(errors.join("\n"));
  if (distinct.length > errors.length) {
    lines.push(`+${distinct.length - errors.length} other error(s)`);
  }

  return {
    kind: "degraded",
    title: `Live check degraded: ${confirmed.length}/${totalStreamers} streamers unreachable`,
    message: lines.join("\n"),
  };
}
