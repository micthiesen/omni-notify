import { Docstore } from "@micthiesen/mitools/docstore";
import { ManagedRuntime } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  deleteEmailFeedback,
  EmailFeedbackEntity,
  formatFeedbackDigest,
  listEmailFeedback,
  recordEmailFeedback,
} from "./feedback.js";

const runtime = ManagedRuntime.make(Docstore.layerMemory);
const runEffect = runtime.runPromise.bind(runtime);

afterEach(async () => {
  await runEffect(EmailFeedbackEntity.deleteAll());
  vi.restoreAllMocks();
});

async function record(
  overrides: Partial<Parameters<typeof recordEmailFeedback>[0]> = {},
) {
  return runEffect(
    recordEmailFeedback({
      pipeline: "ParcelTracker",
      emailId: "e1",
      subject: "Your order shipped",
      from: "orders@shop.com",
      verdict: "not_relevant",
      ...overrides,
    }),
  );
}

describe("recordEmailFeedback", () => {
  it("derives the activityId from pipeline and emailId", async () => {
    const row = await record();
    expect(row.activityId).toBe("ParcelTracker#e1");
    expect(row.createdAt).toBeGreaterThan(0);
  });

  it("upserts: re-recording the same email overwrites the verdict", async () => {
    await record({ verdict: "not_relevant" });
    await record({ verdict: "missed" });
    const rows = await runEffect(listEmailFeedback());
    expect(rows).toHaveLength(1);
    expect(rows[0].verdict).toBe("missed");
  });
});

describe("listEmailFeedback", () => {
  it("returns newest first, filtered by pipeline, capped by limit", async () => {
    const now = vi.spyOn(Date, "now");
    now.mockReturnValue(1_000);
    await record({ emailId: "old" });
    now.mockReturnValue(2_000);
    await record({ emailId: "new" });
    now.mockReturnValue(3_000);
    await record({ pipeline: "CalendarEvents", emailId: "cal" });

    const parcel = await runEffect(listEmailFeedback("ParcelTracker"));
    expect(parcel.map((f) => f.emailId)).toEqual(["new", "old"]);
    expect(await runEffect(listEmailFeedback("CalendarEvents"))).toHaveLength(1);
    expect(await runEffect(listEmailFeedback())).toHaveLength(3);
    expect(await runEffect(listEmailFeedback(undefined, 2))).toHaveLength(2);
  });
});

describe("deleteEmailFeedback", () => {
  it("reports whether the row existed", async () => {
    const row = await record();
    expect(await runEffect(deleteEmailFeedback(row.activityId))).toBe(true);
    expect(await runEffect(deleteEmailFeedback(row.activityId))).toBe(false);
  });
});

describe("formatFeedbackDigest", () => {
  it("returns an empty string when there is no feedback", async () => {
    expect(await runEffect(formatFeedbackDigest("parcel"))).toBe("");
  });

  it("formats not_relevant and missed corrections for the pipeline", async () => {
    await record({ emailId: "e1", verdict: "not_relevant" });
    await record({
      emailId: "e2",
      subject: "Package ready",
      from: "ship@store.com",
      verdict: "missed",
      note: "has a tracking link",
    });
    await record({ pipeline: "CalendarEvents", emailId: "e3", verdict: "missed" });

    const digest = await runEffect(formatFeedbackDigest("parcel"));
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

  it("caps the digest at the given limit", async () => {
    for (let i = 0; i < 5; i++) await record({ emailId: `e${i}` });
    expect(
      (await runEffect(formatFeedbackDigest("parcel", 3))).split("\n"),
    ).toHaveLength(3);
  });
});
