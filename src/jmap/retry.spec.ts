import { describe, expect, it } from "vitest";
import {
  type EmailRetryData,
  MAX_RETRY_ATTEMPTS,
  retryDelayMs,
  selectDueRetries,
} from "./retry.js";

const NOW = 1_800_000_000_000;

function makeRetry(overrides: Partial<EmailRetryData>): EmailRetryData {
  return {
    retryKey: "ParcelTracker#email-1",
    pipeline: "ParcelTracker",
    emailId: "email-1",
    reason: "Parcel API 503",
    attempts: 1,
    nextAttemptAt: NOW - 1,
    createdAt: NOW - 60_000,
    ...overrides,
  };
}

describe("selectDueRetries", () => {
  it("returns rows whose nextAttemptAt has passed", () => {
    const rows = [
      makeRetry({ retryKey: "a", nextAttemptAt: NOW - 1 }),
      makeRetry({ retryKey: "b", nextAttemptAt: NOW }),
      makeRetry({ retryKey: "c", nextAttemptAt: NOW + 1 }),
    ];
    const due = selectDueRetries(rows, NOW);
    expect(due.map((r) => r.retryKey)).toEqual(["a", "b"]);
  });

  it("excludes rows that exhausted their attempts", () => {
    const rows = [
      makeRetry({ retryKey: "ok", attempts: MAX_RETRY_ATTEMPTS }),
      makeRetry({ retryKey: "done", attempts: MAX_RETRY_ATTEMPTS + 1 }),
    ];
    expect(selectDueRetries(rows, NOW).map((r) => r.retryKey)).toEqual(["ok"]);
  });

  it("sorts due rows oldest-first by nextAttemptAt", () => {
    const rows = [
      makeRetry({ retryKey: "later", nextAttemptAt: NOW - 1 }),
      makeRetry({ retryKey: "earlier", nextAttemptAt: NOW - 100 }),
    ];
    expect(selectDueRetries(rows, NOW).map((r) => r.retryKey)).toEqual([
      "earlier",
      "later",
    ]);
  });
});

describe("retryDelayMs", () => {
  it("doubles the delay per attempt starting at 30 minutes", () => {
    expect(retryDelayMs(1)).toBe(30 * 60_000);
    expect(retryDelayMs(2)).toBe(60 * 60_000);
    expect(retryDelayMs(3)).toBe(120 * 60_000);
  });

  it("treats attempt 0 like the first attempt", () => {
    expect(retryDelayMs(0)).toBe(30 * 60_000);
  });
});
