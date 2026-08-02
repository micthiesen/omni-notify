import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { getLastDispatchedAt } from "./persistence.js";

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
 * Guards against silent JMAP pipeline outages (a June incident went 16 days
 * unnoticed): if no email batch has been dispatched for 72 hours, warn loudly
 * (warns reach Pushover via mitools).
 */
export default class EmailWatchdogTask extends ScheduledTask {
  public readonly name = "EmailWatchdog";
  public readonly schedule = "0 0 */6 * * *"; // Every 6 hours

  private readonly logger: Logger;
  private readonly bootedAt: number;
  private lastRunSummary: string | undefined;

  constructor(logger: Logger) {
    super();
    this.logger = logger.extend("EmailWatchdog");
    this.bootedAt = Date.now();
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }

  public async run(): Promise<void> {
    const lastDispatchedAt = getLastDispatchedAt();
    const now = Date.now();

    if (shouldWarn(lastDispatchedAt, this.bootedAt, now)) {
      const since =
        lastDispatchedAt !== undefined
          ? new Date(lastDispatchedAt).toISOString()
          : `boot at ${new Date(this.bootedAt).toISOString()}`;
      this.lastRunSummary = `Stuck: no dispatch since ${since}`;
      this.logger.warn(
        `No email has been dispatched since ${since} — the JMAP pipeline may be stuck`,
      );
      return;
    }

    if (lastDispatchedAt === undefined) {
      this.lastRunSummary = "No dispatch since boot yet (within threshold)";
      this.logger.info(
        "No email dispatched since boot yet (still within watchdog threshold)",
      );
      return;
    }
    this.lastRunSummary = `Healthy: last dispatch ${new Date(lastDispatchedAt).toISOString()}`;
    this.logger.info(
      `Email pipeline healthy: last dispatch at ${new Date(lastDispatchedAt).toISOString()}`,
    );
  }
}
