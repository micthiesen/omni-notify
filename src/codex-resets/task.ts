import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Clock, Effect } from "effect";
import { ResetAlertTask } from "../reset-alerts/task.js";
import { addCompletedHistoryAlerts } from "./history.js";
import { isFreshFeed, selectResetAlerts } from "./policy.js";
import { readResetSources, ResetSourceError } from "./source.js";

const readSnapshot = Effect.gen(function* () {
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
  return {
    alerts,
    now,
    metadata: {
      feedGeneratedAt: feed.generatedAt,
      feedItems: feed.items.length,
      historyItems: history.items.length,
      completedHistoryFallbacks: combined.items.length - feed.items.length,
      newestAlertId: newest?.id,
      sourcePublishedAt: newest?.sourcePublishedAt,
      alertPublishedAt: newest?.publishedAt,
    },
  };
});

export class CodexResetTask extends ResetAlertTask {
  public constructor(logger: NamedLogger) {
    super(
      "CodexResets",
      "Codex Reset Alerts",
      "codex",
      "Reset Beacon",
      readSnapshot,
      logger,
    );
  }
}
