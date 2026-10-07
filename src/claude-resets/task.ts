import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Clock, Effect } from "effect";
import { ResetAlertTask } from "../reset-alerts/task.js";
import { selectClaudeResetAlerts } from "./policy.js";
import { readClaudeResetSource } from "./source.js";

const readSnapshot = Effect.gen(function* () {
  const feed = yield* readClaudeResetSource();
  const now = yield* Clock.currentTimeMillis;
  const newest = [...feed.events].sort(
    (a, b) => Date.parse(b.date) - Date.parse(a.date),
  )[0];
  return {
    alerts: selectClaudeResetAlerts(feed, now),
    now,
    metadata: {
      catalogUpdatedAt: feed.updated,
      feedItems: feed.events.length,
      newestEventId: newest?.id,
      eventDate: newest?.date,
    },
  };
});

export class ClaudeResetTask extends ResetAlertTask {
  public constructor(logger: NamedLogger) {
    super(
      "ClaudeResets",
      "Claude Code Reset Alerts",
      "claude",
      "Reset Radar",
      readSnapshot,
      logger,
    );
  }
}
