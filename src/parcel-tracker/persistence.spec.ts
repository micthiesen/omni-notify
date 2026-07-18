import { describe, expect, it } from "vitest";
import { deriveItemsOutcome } from "../jmap/activity.js";
import { findNearDuplicateTracking } from "./persistence.js";

describe("findNearDuplicateTracking", () => {
  it("matches an exactly equal known number", () => {
    expect(findNearDuplicateTracking("P5253806501", ["P5253806501"])).toBe(
      "P5253806501",
    );
  });

  it("matches when the candidate contains a known number (both >= 8 chars)", () => {
    expect(findNearDuplicateTracking("P5253806501", ["P52538065"])).toBe("P52538065");
  });

  it("matches when a known number contains the candidate (both >= 8 chars)", () => {
    expect(findNearDuplicateTracking("P52538065", ["P5253806501"])).toBe("P5253806501");
  });

  it("does not containment-match when the candidate is shorter than 8 chars", () => {
    expect(findNearDuplicateTracking("P525380", ["P5253806501"])).toBeUndefined();
  });

  it("does not containment-match when the known number is shorter than 8 chars", () => {
    expect(findNearDuplicateTracking("P5253806501", ["P525380"])).toBeUndefined();
  });

  it("still matches short strings when exactly equal", () => {
    expect(findNearDuplicateTracking("ABC123", ["ABC123"])).toBe("ABC123");
  });

  it("returns undefined for unrelated numbers", () => {
    expect(
      findNearDuplicateTracking("1Z999AA10123456784", ["P5253806501", "9400111899"]),
    ).toBeUndefined();
  });

  it("returns undefined for an empty known set", () => {
    expect(findNearDuplicateTracking("P5253806501", [])).toBeUndefined();
  });

  it("returns the first matching known number", () => {
    expect(findNearDuplicateTracking("P5253806501", ["P52538065", "P5253806501"])).toBe(
      "P52538065",
    );
  });
});

// Parcel per-item outcome semantics: submitted / already-submitted /
// near-duplicate count as ok; rejections and unavailable-carrier-list do not.
describe("parcel outcome derivation", () => {
  it("derives processed when every item succeeded", () => {
    expect(deriveItemsOutcome([true, true])).toBe("processed");
  });

  it("derives processed when items were dedup-skipped (treated as ok)", () => {
    expect(deriveItemsOutcome([true])).toBe("processed");
  });

  it("derives partial when some items failed", () => {
    expect(deriveItemsOutcome([true, false])).toBe("partial");
  });

  it("derives failed when every item failed", () => {
    expect(deriveItemsOutcome([false, false])).toBe("failed");
  });

  it("derives no_matches for an empty extraction", () => {
    expect(deriveItemsOutcome([])).toBe("no_matches");
  });
});
