import { describe, expect, it } from "vitest";
import { shouldWarn, WATCHDOG_THRESHOLD_MS } from "./watchdogTask.js";

const HOUR = 60 * 60_000;
const NOW = 1_800_000_000_000;
const BOOT = NOW - 100 * HOUR;

describe("shouldWarn", () => {
  it("does not warn when a dispatch happened recently", () => {
    expect(shouldWarn(NOW - HOUR, BOOT, NOW)).toBe(false);
  });

  it("warns when the last dispatch is older than the threshold", () => {
    expect(shouldWarn(NOW - 73 * HOUR, BOOT, NOW)).toBe(true);
  });

  it("does not warn exactly at the threshold boundary", () => {
    expect(shouldWarn(NOW - WATCHDOG_THRESHOLD_MS, BOOT, NOW)).toBe(false);
  });

  it("uses boot time when nothing was ever dispatched", () => {
    expect(shouldWarn(undefined, NOW - 10 * HOUR, NOW)).toBe(false);
    expect(shouldWarn(undefined, NOW - 73 * HOUR, NOW)).toBe(true);
  });

  it("respects a custom threshold", () => {
    expect(shouldWarn(NOW - 2 * HOUR, BOOT, NOW, HOUR)).toBe(true);
    expect(shouldWarn(NOW - 2 * HOUR, BOOT, NOW, 3 * HOUR)).toBe(false);
  });
});
