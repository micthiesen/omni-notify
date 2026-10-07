import { describe, expect, it } from "vitest";
import { selectClaudeResetAlerts } from "./policy.js";
import type { ClaudeResetSource } from "./source.js";

const now = Date.parse("2026-10-07T16:00:00Z");
type Event = ClaudeResetSource["events"][number];
const event: Event = {
  id: "october-reset",
  date: "2026-10-07T15:00:00Z",
  type: "counter-reset",
  status: "historic",
  confidence: "confirmed",
  plans: ["all"],
  surfaces: ["claude-code", "claude-app"],
  title: "Usage counters reset",
  summary: "The tracker reports a reset.",
  sources: [{ url: "https://x.com/ClaudeDevs/status/123" }],
};
const source = (events: Event[] = [event]): ClaudeResetSource => ({
  updated: "2026-10-07",
  events,
});

describe("Claude reset alert policy", () => {
  it("reports a reset without claiming the user's account was verified", () => {
    const [alert] = selectClaudeResetAlerts(source(), now);
    expect(alert.title).toBe("Claude Code reset reported");
    expect(alert.message).toContain("Your account has not been verified");
    expect(alert.message).toContain("Source: Reset Radar");
    expect(alert.url).toBe(event.sources[0].url);
  });

  it("preserves banked wording without inventing an automatic refill or expiry", () => {
    const [alert] = selectClaudeResetAlerts(
      source([
        {
          ...event,
          title: "A reset to bank and spend later",
          summary: "Subscribers can save this reset until they choose to redeem it.",
        },
      ]),
      now,
    );
    expect(alert.message).toContain("bank and spend later");
    expect(alert.message).toContain("Settings → Usage");
    expect(alert.message).toContain("Banked resets refill usage only when redeemed");
    expect(alert.message).not.toContain("Non-banked");
    expect(alert.message).not.toContain("Oct 22");
  });

  it("fails closed for policies, projections, upcoming and unconfirmed reports", () => {
    for (const patch of [
      { type: "policy-change" },
      { type: "unknown" },
      { status: "projected" },
      { status: "upcoming" },
      { confidence: "uncertain" },
      { confidence: "projected" },
      { surfaces: ["claude-app"] },
    ])
      expect(selectClaudeResetAlerts(source([{ ...event, ...patch }]), now)).toEqual(
        [],
      );
  });

  it("uses event age, never catalog update or revision, to bound replay", () => {
    for (const date of [
      "2026-10-05T15:59:59Z",
      "2026-10-07T16:05:01Z",
      "2026-09-22T16:31:00Z",
    ]) {
      expect(selectClaudeResetAlerts(source([{ ...event, date }]), now)).toEqual([]);
    }
    expect(
      selectClaudeResetAlerts(
        source([{ ...event, date: "2026-10-05T16:00:00Z" }]),
        now,
      ),
    ).toHaveLength(1);
  });

  it("keeps stable event and post identities through cosmetic revisions", () => {
    const [original] = selectClaudeResetAlerts(source(), now);
    const revisedEvent = {
      ...event,
      title: "Revised wording",
      sources: [
        { url: "https://www.twitter.com/newhandle/status/123/photo/1?s=20#content" },
      ],
    };
    const [revised] = selectClaudeResetAlerts(source([revisedEvent]), now);
    expect(revised.key).toBe(original.key);
    expect(revised.aliases).toEqual(original.aliases);
    expect(
      selectClaudeResetAlerts(
        source([event, { ...revisedEvent, id: "new-catalog-id" }]),
        now,
      ),
    ).toHaveLength(1);
  });

  it("does not deduplicate unrelated events sharing a background source", () => {
    const shared = { url: "https://www.anthropic.com/news" };
    expect(
      selectClaudeResetAlerts(
        source([
          { ...event, sources: [...event.sources, shared] },
          {
            ...event,
            id: "different",
            sources: [{ url: "https://x.com/ClaudeDevs/status/456" }, shared],
          },
        ]),
        now,
      ),
    ).toHaveLength(2);
  });

  it("keeps source query and fragment identities and falls back to tracker details", () => {
    const alerts = selectClaudeResetAlerts(
      source([
        { ...event, sources: [{ url: "https://example.com/?id=1#post" }] },
        {
          ...event,
          id: "second",
          sources: [{ url: "https://example.com/?id=2#post" }],
        },
        { ...event, id: "third", sources: [] },
      ]),
      now,
    );
    expect(alerts).toHaveLength(3);
    expect(alerts.find((a) => a.key === "third:reported")?.url).toBe(
      "https://resetradar.com/#third",
    );
  });

  it("bounds noisy summaries and removes URLs from the preview", () => {
    const [alert] = selectClaudeResetAlerts(
      source([
        {
          ...event,
          summary: `See https://example.com ${"word ".repeat(1_000)}`,
        },
      ]),
      now,
    );
    expect(alert.message).not.toContain("https://");
    expect(alert.message.split("\n")[1].length).toBeLessThanOrEqual(200);
  });
});
