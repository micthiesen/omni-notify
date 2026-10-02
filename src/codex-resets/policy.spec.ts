import { describe, expect, it } from "vitest";
import { Schema } from "effect";
import { AlertFeedSchema, type AlertFeed, type ResetHistory } from "./source.js";
import { ALERT_LOOKBACK_MS, FEED_MAX_AGE_MS, selectResetAlerts } from "./policy.js";

const now = Date.parse("2026-10-02T16:00:00Z");
const item: AlertFeed["items"][number] = {
  id: "announcement:revision-1",
  eventId: "october-reset",
  postId: "123",
  topic: "schedule",
  state: "official_scheduled",
  title: "Reset scheduled",
  summary: "An official post names a reset time.",
  sourceUrl: "https://x.com/thsottiaux/status/123",
  evidenceId: "capture-123",
  targetAt: "2026-10-02T17:00:00Z",
  publishedAt: "2026-10-02T15:00:00Z",
  withdrawn: false,
};
const event: ResetHistory["items"][number] = {
  id: "123",
  kind: "special_global",
  scope: "all",
  eventKind: "scheduled",
  evidenceClass: "named_public_source",
  status: "active",
  fulfilledBy: null,
  supersededBy: null,
  sources: [{ announcementId: "123", url: item.sourceUrl }],
};
const feed = (items = [item]): AlertFeed => ({
  generatedAt: new Date(now).toISOString(),
  items,
});
const history = (entry = event): ResetHistory => ({ items: [entry] });

describe("Codex reset alert policy", () => {
  it("distinguishes forecast, schedule, observed rollout and confirmed landing", () => {
    for (const [topic, state, expected] of [
      ["likely", "likely_forecast", "looks likely"],
      ["schedule", "official_scheduled", "announced"],
      ["rollout", "rollout_observed", "landing (observed)"],
      ["action", "action_claimed", "reported landed"],
    ]) {
      const [alert] = selectResetAlerts(
        feed([{ ...item, topic, state }]),
        history(
          topic === "action"
            ? { ...event, eventKind: "completed", status: "completed" }
            : event,
        ),
        now,
      );
      expect(alert.title).toContain(expected);
      expect(alert.message).toContain("Type: non-banked; scope: all");
      expect(alert.message).toContain(item.sourceUrl);
      expect(alert.message).toContain("https://resetbeacon.com/evidence/capture-123/");
    }
  });

  it("never turns an elapsed scheduled time into a landed claim", () => {
    const [alert] = selectResetAlerts(
      feed([{ ...item, targetAt: "2026-10-02T14:00:00Z" }]),
      history(),
      now,
    );
    expect(alert.title).toContain("announced");
    expect(alert.message).toContain("Time has passed; landing is not yet confirmed");
    expect(alert.message).toContain("7:00 a.m. PDT");
  });

  it("labels banked credits without claiming current usage was restored", () => {
    const [alert] = selectResetAlerts(
      feed([{ ...item, topic: "action", state: "action_claimed" }]),
      history({
        ...event,
        kind: "banked",
        evidenceClass: "measured_account",
        status: "recorded",
        eventKind: "policy_change",
      }),
      now,
    );
    expect(alert.title).toContain("(banked)");
    expect(alert.message).toContain("current usage is unchanged until redeemed");
  });

  it("does not infer a reset type without matching history evidence", () => {
    const [alert] = selectResetAlerts(feed(), { items: [] }, now);
    expect(alert.message).toContain("Type: unspecified");
  });

  it("does not promote a banked policy promise mislabeled action_claimed to landed", () => {
    const [alert] = selectResetAlerts(
      feed([
        {
          ...item,
          topic: "action",
          state: "action_claimed",
          summary:
            "Correction: it had not landed. The first banked reset will arrive in three hours.",
        },
      ]),
      history({
        ...event,
        kind: "banked",
        eventKind: "policy_change",
        status: "recorded",
      }),
      now,
    );
    expect(alert.title).toBe("Codex reset update (banked)");
    expect(alert.message).toContain("it had not landed");
  });

  it("rejects stale/future snapshots and ignores old/future/withdrawn signals", () => {
    for (const generatedAt of [now - FEED_MAX_AGE_MS - 1, now + 6 * 60_000]) {
      expect(
        selectResetAlerts(
          { ...feed(), generatedAt: new Date(generatedAt).toISOString() },
          history(),
          now,
        ),
      ).toEqual([]);
    }
    const items = [
      { ...item, withdrawn: true },
      { ...item, publishedAt: new Date(now - ALERT_LOOKBACK_MS - 1).toISOString() },
      { ...item, publishedAt: new Date(now + 6 * 60_000).toISOString() },
    ];
    expect(selectResetAlerts(feed(items), history(), now)).toEqual([]);
  });

  it("suppresses fulfilled, superseded and expired previews", () => {
    for (const status of [
      "fulfilled",
      "completed",
      "expired",
      "missed",
      "superseded",
    ]) {
      expect(selectResetAlerts(feed(), history({ ...event, status }), now)).toEqual([]);
    }
    expect(
      selectResetAlerts(feed(), history({ ...event, fulfilledBy: "new-reset" }), now),
    ).toEqual([]);
  });

  it("sends only the current stage on enrollment and deduplicates revisions", () => {
    const likely = { ...item, topic: "likely", state: "likely_forecast" };
    expect(
      selectResetAlerts(
        feed([likely, item, { ...item, id: "revision-2" }]),
        history(),
        now,
      ),
    ).toHaveLength(1);
    const action = { ...item, topic: "action", state: "action_claimed" };
    const alerts = selectResetAlerts(
      feed([likely, item, action]),
      history({ ...event, eventKind: "completed", status: "completed" }),
      now,
    );
    expect(alerts).toHaveLength(1);
    expect(alerts[0].title).toContain("reported landed");
  });

  it("retains distinct keys for schedule/type changes and stages", () => {
    const first = selectResetAlerts(feed(), history(), now)[0];
    const changed = selectResetAlerts(
      feed([{ ...item, targetAt: "2026-10-03T17:00:00Z" }]),
      history(),
      now,
    )[0];
    const banked = selectResetAlerts(
      feed(),
      history({ ...event, kind: "banked" }),
      now,
    )[0];
    expect(first.key).not.toEqual(changed.key);
    expect(first.key).not.toEqual(banked.key);
  });

  it("bounds notification length while preserving the source link", () => {
    const [alert] = selectResetAlerts(
      feed([
        {
          ...item,
          summary: "A".repeat(16_000),
          sourceUrl: `https://example.com/${"x".repeat(490)}`,
        },
      ]),
      history(),
      now,
    );
    expect(alert.message.length).toBeLessThanOrEqual(1024);
    expect(alert.message).toContain("https://example.com/");
  });

  it("fails source decoding on invalid timestamps, unsafe URLs and missing fields", () => {
    const decode = Schema.decodeUnknownSync(AlertFeedSchema);
    expect(() => decode({ ...feed(), generatedAt: "not a date" })).toThrow();
    expect(() =>
      decode(feed([{ ...item, sourceUrl: "javascript:alert(1)" }])),
    ).toThrow();
    expect(() => decode({ generatedAt: new Date(now).toISOString() })).toThrow();
    expect(decode(feed())).toEqual(feed());
  });
});
