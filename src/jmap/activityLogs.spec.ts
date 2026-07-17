import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { installLogCapture } from "../task-runs/logCapture.js";
import {
  EmailActivityLogEntity,
  getEmailActivityLogs,
  saveEmailActivityLogs,
  withEmailLogCapture,
} from "./activityLogs.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "activitylogs.spec.db",
  },
});

const logger = new Logger("Test");

beforeAll(() => {
  installLogCapture();
  vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(() => {
  EmailActivityLogEntity.deleteAll();
});

describe("email activity log capture", () => {
  it("captures lines logged during processing and persists them", async () => {
    const result = await withEmailLogCapture(
      "ParcelTracker#e1",
      "ParcelTracker",
      async () => {
        logger.info("extracting");
        await Promise.resolve();
        logger.info("submitted");
        return 42;
      },
    );

    expect(result).toBe(42);
    const stored = getEmailActivityLogs("ParcelTracker#e1");
    expect(stored?.lines.map((l) => l.msg)).toEqual(["extracting", "submitted"]);
    expect(stored?.dropped).toBe(0);
  });

  it("persists no row when nothing was logged", async () => {
    await withEmailLogCapture("ParcelTracker#e2", "ParcelTracker", async () => {});
    expect(getEmailActivityLogs("ParcelTracker#e2")).toBeUndefined();
  });

  it("deletes a stale row when a reprocess captures nothing", async () => {
    saveEmailActivityLogs({
      activityId: "ParcelTracker#e3",
      lines: [{ t: 1, level: LogLevel.INFO, logger: "Test", msg: "old" }],
      dropped: 0,
    });
    expect(getEmailActivityLogs("ParcelTracker#e3")).toBeDefined();

    await withEmailLogCapture("ParcelTracker#e3", "ParcelTracker", async () => {});
    expect(getEmailActivityLogs("ParcelTracker#e3")).toBeUndefined();
  });

  it("still persists the capture when fn throws", async () => {
    await expect(
      withEmailLogCapture("ParcelTracker#e4", "ParcelTracker", async () => {
        logger.info("before failure");
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");

    expect(getEmailActivityLogs("ParcelTracker#e4")?.lines.map((l) => l.msg)).toEqual([
      "before failure",
    ]);
  });
});
