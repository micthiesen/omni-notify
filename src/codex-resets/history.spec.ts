import { describe, expect, it } from "vitest";
import { addCompletedHistoryAlerts } from "./history.js";
import type { AlertFeed, ResetHistory } from "./source.js";

const now = Date.parse("2026-10-02T12:00:00Z");
const announcedAt = "2026-10-02T11:00:00Z";
const feed: AlertFeed = { generatedAt: new Date(now).toISOString(), items: [] };
const completedEvent = (overrides: Record<string, unknown> = {}) => ({
  id: "completion-1",
  kind: "special_global",
  scope: "all",
  eventKind: "completed",
  evidenceClass: "reported",
  status: "completed",
  fulfilledBy: null,
  supersededBy: null,
  announcedAt,
  summary: "The reset completed.",
  sources: [
    { announcementId: "post-completion", url: "https://resetbeacon.com/post/1" },
  ],
  ...overrides,
});
const history = (...items: ResetHistory["items"]): ResetHistory => ({ items });
const scheduledParent = (completionId: string): ResetHistory["items"][number] => ({
  id: `parent-${completionId}`,
  kind: "special_global",
  scope: "all",
  eventKind: "scheduled",
  evidenceClass: "named_public_source",
  status: "fulfilled",
  fulfilledBy: completionId,
  supersededBy: null,
  sources: [
    {
      announcementId: `post-parent-${completionId}`,
      url: "https://resetbeacon.com/parent",
    },
  ],
});
const linkedHistory = (event: ResetHistory["items"][number]) =>
  history(scheduledParent(event.id), event);
const withParent = (base: AlertFeed, completionId = "completion-1"): AlertFeed => ({
  ...base,
  items: [
    {
      id: "parent-feed",
      eventId: "existing-event",
      postId: `post-parent-${completionId}`,
      topic: "schedule",
      state: "official_scheduled",
      title: "Scheduled",
      summary: "Earlier announcement",
      sourceUrl: "https://resetbeacon.com/parent",
      evidenceId: null,
      targetAt: announcedAt,
      publishedAt: announcedAt,
      withdrawn: false,
    },
    ...base.items,
  ],
});

describe("addCompletedHistoryAlerts", () => {
  it("adds a recent completed global reset with its source announcement", () => {
    const result = addCompletedHistoryAlerts(
      withParent(feed),
      linkedHistory(completedEvent()),
      now,
    );
    expect(result.generatedAt).toBe(feed.generatedAt);
    expect(result.items.slice(1)).toEqual([
      expect.objectContaining({
        id: "history:completion-1",
        eventId: "existing-event",
        postId: "post-completion",
        topic: "action",
        state: "action_claimed",
        sourceUrl: "https://resetbeacon.com/post/1",
        targetAt: null,
        evidenceId: null,
        publishedAt: announcedAt,
        sourcePublishedAt: announcedAt,
        summary: "The reset completed.",
        withdrawn: false,
      }),
    ]);
  });

  it("omits stale, too-far-future, and non-completed history", () => {
    const cases = [
      completedEvent({ id: "stale", announcedAt: "2026-09-30T11:59:59Z" }),
      completedEvent({ id: "future", announcedAt: "2026-10-02T12:05:01Z" }),
      completedEvent({
        id: "preview",
        eventKind: "policy_change",
        status: "announced",
      }),
      completedEvent({ id: "banked", kind: "banked" }),
      completedEvent({ id: "expired", status: "expired" }),
      completedEvent({ id: "superseded", supersededBy: "replacement" }),
      completedEvent({ id: "unlinked", scope: "pro" }),
    ];
    for (const event of cases) {
      const input = withParent(feed, event.id);
      expect(addCompletedHistoryAlerts(input, linkedHistory(event), now).items).toEqual(
        input.items,
      );
    }
  });

  it("requires an explicitly fulfilled scheduled parent", () => {
    const event = completedEvent();
    const input = withParent(feed);
    expect(addCompletedHistoryAlerts(input, history(event), now).items).toEqual(
      input.items,
    );
    const parent = {
      ...scheduledParent(event.id),
      status: "active",
    };
    expect(addCompletedHistoryAlerts(input, history(parent, event), now).items).toEqual(
      input.items,
    );
    expect(
      addCompletedHistoryAlerts(
        input,
        history(
          {
            ...scheduledParent(event.id),
            kind: "banked",
          },
          event,
        ),
        now,
      ).items,
    ).toEqual(input.items);
  });

  it("does not create a new delivery identity if the parent feed item disappeared", () => {
    expect(
      addCompletedHistoryAlerts(feed, linkedHistory(completedEvent()), now).items,
    ).toEqual([]);
  });

  it("does not bypass a withdrawn feed post", () => {
    const withdrawnFeed: AlertFeed = {
      ...feed,
      items: [
        {
          id: "withdrawn",
          eventId: "event-1",
          postId: "post-completion",
          topic: "action",
          state: "action_claimed",
          title: "Old claim",
          summary: "Withdrawn",
          sourceUrl: "https://resetbeacon.com/post/1",
          evidenceId: null,
          targetAt: null,
          publishedAt: announcedAt,
          withdrawn: true,
        },
      ],
    };
    expect(
      addCompletedHistoryAlerts(
        withParent(withdrawnFeed),
        linkedHistory(completedEvent()),
        now,
      ).items,
    ).toHaveLength(2);
  });

  it("adds completion beside an earlier likely or scheduled post", () => {
    const previewFeed: AlertFeed = {
      ...feed,
      items: [
        {
          id: "preview",
          eventId: "event-1",
          postId: "post-completion",
          topic: "schedule",
          state: "official_scheduled",
          title: "Reset scheduled",
          summary: "A reset was announced.",
          sourceUrl: "https://resetbeacon.com/post/1",
          evidenceId: null,
          targetAt: announcedAt,
          publishedAt: announcedAt,
          withdrawn: false,
        },
      ],
    };
    const result = addCompletedHistoryAlerts(
      withParent(previewFeed),
      linkedHistory(completedEvent()),
      now,
    );
    expect(result.items).toHaveLength(3);
    expect(result.items[2]).toMatchObject({
      id: "history:completion-1",
      postId: "post-completion",
    });
  });

  it("checks all announcement sources for a withdrawn or completed feed post", () => {
    const multiSource = completedEvent({
      sources: [
        { announcementId: "post-new", url: "https://resetbeacon.com/post/new" },
        { announcementId: "post-withdrawn", url: "https://resetbeacon.com/post/old" },
      ],
    });
    const withdrawnFeed: AlertFeed = {
      ...feed,
      items: [
        {
          id: "withdrawn",
          eventId: "event-old",
          postId: "post-withdrawn",
          topic: "action",
          state: "action_claimed",
          title: "Old claim",
          summary: "Withdrawn",
          sourceUrl: "https://resetbeacon.com/post/old",
          evidenceId: null,
          targetAt: null,
          publishedAt: announcedAt,
          withdrawn: true,
        },
      ],
    };
    expect(
      addCompletedHistoryAlerts(
        withParent(withdrawnFeed),
        linkedHistory(multiSource),
        now,
      ).items,
    ).toHaveLength(2);
  });

  it("reuses the parent feed event id through fulfilledBy", () => {
    const parent = {
      ...completedEvent({
        id: "parent",
        kind: "special_global",
        scope: "all",
        eventKind: "scheduled",
        status: "fulfilled",
        fulfilledBy: "completion-1",
        announcedAt: undefined,
        sources: [
          { announcementId: "post-parent", url: "https://resetbeacon.com/post/old" },
        ],
      }),
    };
    const parentFeed: AlertFeed = {
      ...feed,
      items: [
        {
          id: "parent-feed-item",
          eventId: "existing-event",
          postId: "post-parent",
          topic: "schedule",
          state: "official_scheduled",
          title: "Scheduled",
          summary: "Earlier announcement",
          sourceUrl: "https://resetbeacon.com/post/old",
          evidenceId: null,
          targetAt: announcedAt,
          publishedAt: announcedAt,
          withdrawn: false,
        },
      ],
    };
    const result = addCompletedHistoryAlerts(
      parentFeed,
      history(parent, completedEvent()),
      now,
    );
    expect(result.items.at(-1)).toMatchObject({
      eventId: "existing-event",
      postId: "post-completion",
    });
  });
});
