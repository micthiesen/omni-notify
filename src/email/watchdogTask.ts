import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Clock, Effect } from "effect";
import { getLastDispatchedAtEffect } from "./persistence.js";
import type { TaskServices } from "../task-runs/registry.js";

export const WATCHDOG_THRESHOLD_MS = 72 * 60 * 60_000;

/**
 * Pure: should the watchdog warn? True when nothing has been dispatched within
 * `thresholdMs` of `now`. When no email has ever been dispatched, boot time
 * stands in for the last dispatch so a fresh install (or wiped DB) still warns
 * once the process has been up past the threshold without any mail.
 */
export function shouldWarn(
  lastDispatchedAt: number | undefined,
  bootedAt: number,
  now: number,
  thresholdMs: number = WATCHDOG_THRESHOLD_MS,
): boolean {
  const reference = lastDispatchedAt ?? bootedAt;
  return now - reference > thresholdMs;
}

/**
 * Guards against silent email pipeline outages (a June incident went 16 days
 * unnoticed): if no email batch has been dispatched for 72 hours, warn loudly
 * (warns reach Pushover via mitools).
 */
export default class EmailWatchdogTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "EmailWatchdog";
  public readonly schedule = "0 0 */6 * * *"; // Every 6 hours

  private readonly logger: NamedLogger;
  private readonly bootedAt: number;
  private lastRunSummary: string | undefined;

  constructor(logger: NamedLogger) {
    this.logger = logger.extend("EmailWatchdog");
    this.bootedAt = Date.now();
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }

  public readonly run = Effect.gen({ self: this }, function* () {
    const lastDispatchedAt = yield* getLastDispatchedAtEffect;
    const now = yield* Clock.currentTimeMillis;

    if (shouldWarn(lastDispatchedAt, this.bootedAt, now)) {
      const since =
        lastDispatchedAt !== undefined
          ? new Date(lastDispatchedAt).toISOString()
          : `boot at ${new Date(this.bootedAt).toISOString()}`;
      this.lastRunSummary = `Stuck: no dispatch since ${since}`;
      yield* this.logger.warn(
        `No email has been dispatched since ${since} — the email pipeline may be stuck`,
      );
      return;
    }

    if (lastDispatchedAt === undefined) {
      this.lastRunSummary = "No dispatch since boot yet (within threshold)";
      yield* this.logger.info(
        "No email dispatched since boot yet (still within watchdog threshold)",
      );
      return;
    }
    this.lastRunSummary = `Healthy: last dispatch ${new Date(lastDispatchedAt).toISOString()}`;
    yield* this.logger.info(
      `Email pipeline healthy: last dispatch at ${new Date(lastDispatchedAt).toISOString()}`,
    );
  });
}
