import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import type { TaskServices } from "../task-runs/registry.js";
import { createResetDelivery, type ResetAlert } from "./delivery.js";

export interface ResetSnapshot {
  readonly alerts: readonly ResetAlert[];
  readonly now: number;
  readonly metadata: Record<string, unknown>;
}

/** Providers own evidence interpretation; scheduling and delivery stay identical. */
export class ResetAlertTask implements ScheduledTask<unknown, TaskServices> {
  public readonly schedule = "0 * * * * *";
  public readonly runOnStartup = true;
  private lastSummary?: string;

  public constructor(
    public readonly name: string,
    public readonly displayName: string,
    private readonly provider: "codex" | "claude",
    private readonly sourceLabel: string,
    private readonly readSnapshot: Effect.Effect<ResetSnapshot, unknown, TaskServices>,
    private readonly logger: NamedLogger,
  ) {}

  public readonly run = Effect.gen({ self: this }, function* () {
    this.lastSummary = undefined;
    const { alerts, now, metadata } = yield* this.readSnapshot;
    yield* this.logger.info("Reset source snapshot", {
      fetchedAt: new Date(now).toISOString(),
      ...metadata,
    });
    const { deliverResetAlerts } = createResetDelivery(this.provider);
    const result = yield* deliverResetAlerts(alerts, now);
    this.lastSummary = `${this.sourceLabel}: ${result.sent} sent, ${result.skipped} already handled, ${result.uncertain} uncertain; ${alerts.length} current signals`;
    yield* this.logger.info(this.lastSummary);
  });

  public getLastRunSummary(): string | undefined {
    return this.lastSummary;
  }
}
