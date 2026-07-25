import { Logger, type LogHook } from "@micthiesen/mitools/logging";
import { formatElapsed } from "../utils/dates.js";

/**
 * Cooldown after the Nth delivery of an alert key (index = deliveries so far).
 * A repeating error therefore alerts immediately, then at most every 15min,
 * 30min, 1h, and finally every 3h for as long as it keeps firing.
 */
const DEFAULT_COOLDOWNS_MS = [15 * 60_000, 30 * 60_000, 60 * 60_000, 3 * 60 * 60_000];

/** Silence after which a key is considered a fresh incident again. */
const DEFAULT_RESET_MS = 6 * 60 * 60_000;

/** Ceiling on tracked keys; least-recently-seen entries are evicted first. */
const DEFAULT_MAX_KEYS = 500;

export interface ThrottledAlert {
  key: string;
  title: string;
  body: string;
}

interface AlertEntry {
  lastSentAt: number;
  deliveries: number;
  suppressed: number;
}

export interface AlertThrottleOptions {
  cooldownsMs?: number[];
  resetMs?: number;
  maxKeys?: number;
}

/**
 * Collapses a repeating alert into one delivery plus rarer and rarer reminders.
 *
 * The counted-repeat message matters as much as the suppression: an error that
 * fires every 20s should read as "still broken, 89 times since" rather than as
 * 89 separate notifications.
 */
export class AlertThrottle {
  private readonly cooldownsMs: number[];
  private readonly resetMs: number;
  private readonly maxKeys: number;
  /** Insertion order is maintained as LRU: every touch re-inserts its key. */
  private readonly entries = new Map<string, AlertEntry>();

  public constructor(options: AlertThrottleOptions = {}) {
    this.cooldownsMs = options.cooldownsMs ?? DEFAULT_COOLDOWNS_MS;
    this.resetMs = options.resetMs ?? DEFAULT_RESET_MS;
    this.maxKeys = options.maxKeys ?? DEFAULT_MAX_KEYS;
  }

  /**
   * Returns the alert to deliver (body annotated with the repeat count when
   * occurrences were swallowed in between), or null when it should be dropped.
   */
  public admit(
    alert: ThrottledAlert,
    now: number,
  ): { title: string; body: string } | null {
    const existing = this.touch(alert.key, now);
    if (!existing) {
      this.entries.set(alert.key, { lastSentAt: now, deliveries: 1, suppressed: 0 });
      this.evictOverflow();
      return { title: alert.title, body: alert.body };
    }

    const cooldownMs =
      this.cooldownsMs[Math.min(existing.deliveries - 1, this.cooldownsMs.length - 1)];
    if (now - existing.lastSentAt < cooldownMs) {
      existing.suppressed++;
      return null;
    }

    const sinceMs = now - existing.lastSentAt;
    const suppressed = existing.suppressed;
    existing.lastSentAt = now;
    existing.deliveries++;
    existing.suppressed = 0;
    if (suppressed === 0) return { title: alert.title, body: alert.body };
    return {
      title: alert.title,
      body: `${alert.body}\n\nRepeated ${suppressed + 1} times in the last ${formatElapsed(sinceMs)}.`,
    };
  }

  /**
   * Fetch the live entry for a key and move it to the newest end so eviction
   * drops dormant keys, not the busiest ones. Entries that have gone quiet for
   * longer than resetMs are discarded so the next occurrence reads as a fresh
   * incident.
   */
  private touch(key: string, now: number): AlertEntry | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.entries.delete(key);
    if (now - entry.lastSentAt >= this.resetMs) return undefined;
    this.entries.set(key, entry);
    return entry;
  }

  private evictOverflow(): void {
    while (this.entries.size > this.maxKeys) {
      const oldest = this.entries.keys().next();
      if (oldest.done) break;
      this.entries.delete(oldest.value);
    }
  }
}

/**
 * Dedup key for an alert. Only case and whitespace are normalized: messages
 * that differ by an embedded identifier (tracking number, article URL, email
 * subject) are genuinely distinct incidents and must not be collapsed into one
 * another. Callers whose message carries its own varying counter are expected
 * to throttle at the source instead.
 */
export function alertKey(loggerName: string, title: string): string {
  const normalized = title.toLowerCase().replace(/\s+/g, " ").trim().slice(0, 200);
  return `${loggerName}|${normalized}`;
}

/**
 * Wrap the Logger notification hooks so a repeating failure can't fill
 * Pushover. Call once at boot, after any hook that should stay in the chain has
 * been installed. Deliberately in-memory: containers restart rarely enough that
 * re-alerting once after a restart is the right trade.
 */
export function installAlertThrottle(options?: AlertThrottleOptions): void {
  const throttle = new AlertThrottle(options);
  Logger.onError = wrapHook(Logger.onError, throttle);
  // No-op while mitools leaves onWarn unset (warns don't notify today); wrapped
  // so enabling warn notifications later can't reintroduce the spam.
  Logger.onWarn = wrapHook(Logger.onWarn, throttle);
}

function wrapHook(inner: LogHook | null, throttle: AlertThrottle): LogHook | null {
  if (!inner) return inner;
  return (notification) => {
    const admitted = throttle.admit(
      {
        key: alertKey(notification.loggerName, notification.title),
        title: notification.title,
        body: notification.body,
      },
      Date.now(),
    );
    if (!admitted) return;
    return inner({ ...notification, body: admitted.body });
  };
}
