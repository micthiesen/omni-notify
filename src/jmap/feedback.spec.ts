import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  deleteEmailFeedback,
  EmailFeedbackEntity,
  formatFeedbackDigest,
  listEmailFeedback,
  recordEmailFeedback,
} from "./feedback.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "emailfeedback.spec.db",
  },
});

afterEach(() => {
  EmailFeedbackEntity.deleteAll();
  vi.restoreAllMocks();
});

function record(
  overrides: Partial<Parameters<typeof recordEmailFeedback>[0]> = {},
): ReturnType<typeof recordEmailFeedback> {
  return recordEmailFeedback({
    pipeline: "ParcelTracker",
    emailId: "e1",
    subject: "Your order shipped",
    from: "orders@shop.com",
    verdict: "not_relevant",
    ...overrides,
  });
}

describe("recordEmailFeedback", () => {
  it("derives the activityId from pipeline and emailId", () => {
    const row = record();
    expect(row.activityId).toBe("ParcelTracker#e1");
    expect(row.createdAt).toBeGreaterThan(0);
  });

  it("upserts: re-recording the same email overwrites the verdict", () => {
    record({ verdict: "not_relevant" });
    record({ verdict: "missed" });
    const rows = listEmailFeedback();
    expect(rows).toHaveLength(1);
    expect(rows[0].verdict).toBe("missed");
  });
});

describe("listEmailFeedback", () => {
  it("returns newest first, filtered by pipeline, capped by limit", () => {
    const now = vi.spyOn(Date, "now");
    now.mockReturnValue(1_000);
    record({ emailId: "old" });
    now.mockReturnValue(2_000);
    record({ emailId: "new" });
    now.mockReturnValue(3_000);
    record({ pipeline: "CalendarEvents", emailId: "cal" });

    const parcel = listEmailFeedback("ParcelTracker");
    expect(parcel.map((f) => f.emailId)).toEqual(["new", "old"]);
    expect(listEmailFeedback("CalendarEvents")).toHaveLength(1);
    expect(listEmailFeedback()).toHaveLength(3);
    expect(listEmailFeedback(undefined, 2)).toHaveLength(2);
  });
});

describe("deleteEmailFeedback", () => {
  it("reports whether the row existed", () => {
    const row = record();
    expect(deleteEmailFeedback(row.activityId)).toBe(true);
    expect(deleteEmailFeedback(row.activityId)).toBe(false);
  });
});

describe("formatFeedbackDigest", () => {
  it("returns an empty string when there is no feedback", () => {
    expect(formatFeedbackDigest("parcel")).toBe("");
  });

  it("formats not_relevant and missed corrections for the pipeline", () => {
    record({ emailId: "e1", verdict: "not_relevant" });
    record({
      emailId: "e2",
      subject: "Package ready",
      from: "ship@store.com",
      verdict: "missed",
      note: "has a tracking link",
    });
    record({ pipeline: "CalendarEvents", emailId: "e3", verdict: "missed" });

    const digest = formatFeedbackDigest("parcel");
    expect(digest).toContain(
      '- "Your order shipped" from orders@shop.com: user marked NOT relevant',
    );
    expect(digest).toContain(
      '- "Package ready" from ship@store.com: user marked as MISSED ' +
        "(should have been processed) (note: has a tracking link)",
    );
    // Calendar feedback stays out of the parcel digest
    expect(digest.split("\n")).toHaveLength(2);
  });

  it("caps the digest at the given limit", () => {
    for (let i = 0; i < 5; i++) record({ emailId: `e${i}` });
    expect(formatFeedbackDigest("parcel", 3).split("\n")).toHaveLength(3);
  });
});
