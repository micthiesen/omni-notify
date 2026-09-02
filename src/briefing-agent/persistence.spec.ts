import { beforeEach, describe, expect, it } from "vitest";
import { Effect } from "effect";
import { runTest } from "../live-check/testRuntime.js";
import {
  addBriefingNotification,
  BriefingDeliveryEntity,
  BriefingHistoryEntity,
  completeBriefingDelivery,
  distributeBriefingRunCost,
  formatNotifications,
  getBriefingHistory,
  releaseBriefingDelivery,
  reserveBriefingDelivery,
  resolveHistoryPlaceholders,
  type BriefingNotification,
} from "./persistence.js";

const makeNotification = (
  title: string,
  url = "https://example.com",
): BriefingNotification => ({
  title,
  message: "msg",
  url,
  timestamp: new Date("2026-02-06T14:30:00").getTime(),
});
beforeEach(async () => {
  await runTest(BriefingHistoryEntity.deleteAll());
  await runTest(BriefingDeliveryEntity.deleteAll());
});

describe("formatNotifications", () => {
  it("formats and bounds recent notifications", () => {
    expect(formatNotifications([], 5)).toBe("- No previous notifications");
    const result = formatNotifications(
      [makeNotification("Old"), makeNotification("Recent")],
      1,
    );
    expect(result).not.toContain("Old");
    expect(result).toContain("Recent");
  });
});

describe("briefing persistence", () => {
  it("appends and prunes notification history", async () => {
    for (let index = 0; index < 55; index++)
      await runTest(addBriefingNotification("News", makeNotification(`N${index}`)));
    const history = await runTest(getBriefingHistory("News"));
    expect(history.notifications).toHaveLength(50);
    expect(history.notifications[0].title).toBe("N5");
  });

  it("preserves concurrent notification appends", async () => {
    await runTest(
      Effect.forEach(
        Array.from({ length: 20 }, (_, index) => index),
        (index) => addBriefingNotification("News", makeNotification(`N${index}`)),
        { concurrency: "unbounded", discard: true },
      ),
    );
    const history = await runTest(getBriefingHistory("News"));
    expect(history.notifications).toHaveLength(20);
    expect(new Set(history.notifications.map(({ title }) => title)).size).toBe(20);
  });

  it("does not lose an append concurrent with cost distribution", async () => {
    await runTest(
      addBriefingNotification("News", {
        ...makeNotification("Costed"),
        runId: "run-1",
      }),
    );
    await runTest(
      Effect.all(
        [
          distributeBriefingRunCost("News", "run-1", 12),
          addBriefingNotification("News", makeNotification("Concurrent")),
        ],
        { concurrency: "unbounded", discard: true },
      ),
    );
    const history = await runTest(getBriefingHistory("News"));
    expect(history.notifications.map(({ title }) => title)).toEqual([
      "Costed",
      "Concurrent",
    ]);
    expect(history.notifications[0]?.costCents).toBe(12);
  });

  it("reserves delivery atomically and permits retry after release", async () => {
    expect(await runTest(reserveBriefingDelivery("News", "run:hash"))).toBe(true);
    await runTest(completeBriefingDelivery("News", "run:hash"));
    expect(await runTest(reserveBriefingDelivery("News", "run:hash"))).toBe(false);
    await runTest(releaseBriefingDelivery("News", "run:hash"));
    expect(await runTest(reserveBriefingDelivery("News", "run:hash"))).toBe(true);
  });

  it("resolves every history placeholder", async () => {
    await runTest(addBriefingNotification("News", makeNotification("Article")));
    const result = await runTest(
      resolveHistoryPlaceholders("A: {{history:3}}\nB: {{history:0}}", "News"),
    );
    expect(result).toContain("Article");
    expect(result).toContain("No previous notifications");
    expect(result).not.toContain("{{history");
  });
});
