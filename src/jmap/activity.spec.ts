import { describe, expect, it } from "vitest";
import { type EmailActivityData, selectActivityToPrune } from "./activity.js";

function makeActivity(overrides: Partial<EmailActivityData>): EmailActivityData {
  return {
    activityId: "ParcelTracker#email-1",
    pipeline: "ParcelTracker",
    emailId: "email-1",
    subject: "Your order shipped",
    from: "shop@example.com",
    receivedAt: 1_000,
    processedAt: 1_000,
    outcome: "processed",
    ...overrides,
  };
}

describe("selectActivityToPrune", () => {
  it("returns nothing when under the cap", () => {
    const all = [
      makeActivity({ activityId: "ParcelTracker#a", processedAt: 1 }),
      makeActivity({ activityId: "ParcelTracker#b", processedAt: 2 }),
    ];
    expect(selectActivityToPrune(all, "ParcelTracker", 5)).toEqual([]);
  });

  it("returns the oldest rows beyond the cap", () => {
    const all = [
      makeActivity({ activityId: "ParcelTracker#a", processedAt: 1 }),
      makeActivity({ activityId: "ParcelTracker#b", processedAt: 3 }),
      makeActivity({ activityId: "ParcelTracker#c", processedAt: 2 }),
    ];
    const pruned = selectActivityToPrune(all, "ParcelTracker", 2);
    expect(pruned.map((a) => a.activityId)).toEqual(["ParcelTracker#a"]);
  });

  it("only prunes rows for the given pipeline", () => {
    const all = [
      makeActivity({ activityId: "ParcelTracker#a", processedAt: 1 }),
      makeActivity({
        activityId: "CalendarEvents#b",
        pipeline: "CalendarEvents",
        processedAt: 2,
      }),
    ];
    expect(selectActivityToPrune(all, "ParcelTracker", 1)).toEqual([]);
    expect(selectActivityToPrune(all, "CalendarEvents", 0)).toHaveLength(1);
  });
});
