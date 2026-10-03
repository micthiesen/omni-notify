import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Clock, Effect } from "effect";
import type { TaskServices } from "../task-runs/registry.js";
import { deliverResetAlerts } from "./delivery.js";
import { addCompletedHistoryAlerts } from "./history.js";
import { isFreshFeed, selectResetAlerts } from "./policy.js";
import { readResetSources, ResetSourceError } from "./source.js";

export class CodexResetTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "CodexResets";
  public readonly displayName = "Codex Reset Alerts";
  public readonly schedule = "0 * * * * *";
  public readonly runOnStartup = true;
  private lastSummary?: string;

  public constructor(private readonly logger: NamedLogger) {}

  public readonly run = Effect.gen({ self: this }, function* () {
    this.lastSummary = undefined;
    const { feed, history } = yield* readResetSources();
    const now = yield* Clock.currentTimeMillis;
    if (!isFreshFeed(feed, now))
      return yield* new ResetSourceError({
        operation: "check alert feed freshness",
        cause: `Feed generated at ${feed.generatedAt}`,
      });
    const combined = addCompletedHistoryAlerts(feed, history, now);
    const alerts = selectResetAlerts(combined, history, now);
    const newest = [...feed.items].sort(
      (a, b) => Date.parse(b.publishedAt) - Date.parse(a.publishedAt),
    )[0];
    yield* this.logger.info("Reset source snapshot", {
      fetchedAt: new Date(now).toISOString(),
      feedGeneratedAt: feed.generatedAt,
      feedItems: feed.items.length,
      historyItems: history.items.length,
      completedHistoryFallbacks: combined.items.length - feed.items.length,
      newestAlertId: newest?.id,
      sourcePublishedAt: newest?.sourcePublishedAt,
      alertPublishedAt: newest?.publishedAt,
    });
    const result = yield* deliverResetAlerts(alerts, now);
    this.lastSummary = `Reset Beacon: ${result.sent} sent, ${result.skipped} already handled, ${result.uncertain} uncertain; ${alerts.length} current signals`;
    yield* this.logger.info(this.lastSummary);
  });

  public getLastRunSummary(): string | undefined {
    return this.lastSummary;
  }
}
