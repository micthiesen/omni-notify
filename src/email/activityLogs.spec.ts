import { Docstore } from "@micthiesen/mitools/docstore";
import { OperationError } from "@micthiesen/mitools/errors";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { taskLogTap } from "../task-runs/logCapture.js";
import {
  EmailActivityLogEntity,
  getEmailActivityLogs,
  saveEmailActivityLogs,
  withEmailLogCaptureEffect,
} from "./activityLogs.js";

const runtime = ManagedRuntime.make(
  Layer.merge(Docstore.layerMemory, Logger.layer({ onLog: taskLogTap })),
);
const runEffect = runtime.runPromise.bind(runtime);

const logger = Logger.named("Test");

beforeAll(() => {
  vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(async () => {
  await runEffect(EmailActivityLogEntity.deleteAll());
});

describe("email activity log capture", () => {
  it("captures lines logged during processing and persists them", async () => {
    const result = await runEffect(
      withEmailLogCaptureEffect("ParcelTracker#e1", "ParcelTracker", () =>
        Effect.gen(function* () {
          yield* logger.info("extracting");
          yield* Effect.yieldNow;
          yield* logger.info("submitted");
          return 42;
        }),
      ),
    );

    expect(result).toBe(42);
    const stored = await runEffect(getEmailActivityLogs("ParcelTracker#e1"));
    expect(stored?.lines.map((l) => l.msg)).toEqual(["extracting", "submitted"]);
    expect(stored?.dropped).toBe(0);
  });

  it("persists no row when nothing was logged", async () => {
    await runEffect(
      withEmailLogCaptureEffect("ParcelTracker#e2", "ParcelTracker", () => Effect.void),
    );
    expect(await runEffect(getEmailActivityLogs("ParcelTracker#e2"))).toBeUndefined();
  });

  it("deletes a stale row when a reprocess captures nothing", async () => {
    await runEffect(
      saveEmailActivityLogs({
        activityId: "ParcelTracker#e3",
        lines: [{ t: 1, level: LogLevel.INFO, logger: "Test", msg: "old" }],
        dropped: 0,
      }),
    );
    expect(await runEffect(getEmailActivityLogs("ParcelTracker#e3"))).toBeDefined();

    await runEffect(
      withEmailLogCaptureEffect("ParcelTracker#e3", "ParcelTracker", () => Effect.void),
    );
    expect(await runEffect(getEmailActivityLogs("ParcelTracker#e3"))).toBeUndefined();
  });

  it("still persists the capture when fn throws", async () => {
    await expect(
      runEffect(
        withEmailLogCaptureEffect("ParcelTracker#e4", "ParcelTracker", () =>
          Effect.gen(function* () {
            yield* logger.info("before failure");
            return yield* Effect.fail(new Error("boom"));
          }),
        ),
      ),
    ).rejects.toThrow("boom");

    expect(
      (await runEffect(getEmailActivityLogs("ParcelTracker#e4")))?.lines.map(
        (l) => l.msg,
      ),
    ).toEqual(["before failure"]);
  });

  it("preserves successful handler acceptance when diagnostic persistence fails", async () => {
    const failure = new Error("database unavailable");
    const upsert = vi
      .spyOn(EmailActivityLogEntity, "upsert")
      .mockImplementationOnce(() =>
        Effect.fail(
          new OperationError({
            source: "docstore",
            operation: "upsert",
            cause: failure,
          }),
        ),
      );

    const result = await runEffect(
      withEmailLogCaptureEffect("ParcelTracker#e5", "ParcelTracker", () =>
        Effect.gen(function* () {
          yield* logger.info("external action completed");
          return "accepted";
        }),
      ),
    );

    expect(result).toBe("accepted");
    upsert.mockRestore();
  });

  it("preserves the original handler failure when diagnostic persistence also fails", async () => {
    const handlerFailure = new Error("submission failed");
    const upsert = vi
      .spyOn(EmailActivityLogEntity, "upsert")
      .mockImplementationOnce(() =>
        Effect.fail(
          new OperationError({
            source: "docstore",
            operation: "upsert",
            cause: new Error("database unavailable"),
          }),
        ),
      );

    const error = await runEffect(
      Effect.flip(
        withEmailLogCaptureEffect("ParcelTracker#e6", "ParcelTracker", () =>
          Effect.gen(function* () {
            yield* logger.info("before handler failure");
            return yield* Effect.fail(handlerFailure);
          }),
        ),
      ),
    );

    expect(error).toBe(handlerFailure);
    upsert.mockRestore();
  });
});
