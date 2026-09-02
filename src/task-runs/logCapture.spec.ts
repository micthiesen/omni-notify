import { LogLevel } from "@micthiesen/mitools/logging";
import { Deferred, Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import { runLogBus } from "./events.js";
import {
  finishRunLogCapture,
  getActiveRunLogs,
  MAX_LINE_LENGTH,
  MAX_LINES_PER_RUN,
  runWithLogCaptureEffect,
  startRunLogCapture,
  taskLogTap,
} from "./logCapture.js";
import { getRunLogs, TaskRunLogEntity } from "./persistence.js";

const mitools = createMitoolsTestRuntime({ onLog: taskLogTap });
const logger = mitools.logger;
afterAll(() => mitools.dispose());

beforeAll(() => {
  vi.spyOn(console, "debug").mockImplementation(() => {});
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(async () => {
  await mitools.run(TaskRunLogEntity.deleteAll());
});

describe("log capture", () => {
  it("attributes lines logged inside the run context, including sub-loggers", async () => {
    startRunLogCapture("run-1", "TaskA");
    await mitools.run(
      runWithLogCaptureEffect(
        "run-1",
        logger
          .info("hello")
          .pipe(Effect.andThen(logger.extend("Sub").warn("careful", { code: 7 }))),
      ),
    );
    const buffer = getActiveRunLogs("run-1");
    expect(buffer?.lines).toHaveLength(2);
    expect(buffer?.lines[0]).toMatchObject({
      level: LogLevel.INFO,
      logger: "Test",
      msg: "hello",
    });
    expect(buffer?.lines[1]).toMatchObject({
      level: LogLevel.WARN,
      logger: "Test:Sub",
      msg: 'careful {"code":7}',
    });
    await mitools.run(finishRunLogCapture("run-1"));
  });

  it("captures DEBUG lines even when the sink threshold is info", async () => {
    startRunLogCapture("run-debug", "TaskA");
    await mitools.run(
      runWithLogCaptureEffect("run-debug", logger.debug("below console threshold")),
    );
    expect(getActiveRunLogs("run-debug")?.lines).toHaveLength(1);
    expect(console.debug).not.toHaveBeenCalled();
    await mitools.run(finishRunLogCapture("run-debug"));
  });

  it("ignores lines logged outside any run context", async () => {
    startRunLogCapture("run-2", "TaskA");
    await mitools.run(logger.info("ambient log"));
    expect(getActiveRunLogs("run-2")?.lines).toHaveLength(0);
    await mitools.run(finishRunLogCapture("run-2"));
  });

  it("keeps concurrent runs separate", async () => {
    startRunLogCapture("run-a", "TaskA");
    startRunLogCapture("run-b", "TaskB");
    await mitools.run(
      Effect.gen(function* () {
        const releaseA = yield* Deferred.make<void>();
        yield* Effect.all(
          [
            runWithLogCaptureEffect(
              "run-a",
              logger
                .info("from A")
                .pipe(
                  Effect.andThen(Deferred.await(releaseA)),
                  Effect.andThen(logger.info("from A again")),
                ),
            ),
            runWithLogCaptureEffect(
              "run-b",
              logger
                .info("from B")
                .pipe(Effect.andThen(Deferred.succeed(releaseA, undefined))),
            ),
          ],
          { concurrency: "unbounded" },
        );
      }),
    );
    expect(getActiveRunLogs("run-a")?.lines.map((line) => line.msg)).toEqual([
      "from A",
      "from A again",
    ]);
    expect(getActiveRunLogs("run-b")?.lines.map((line) => line.msg)).toEqual([
      "from B",
    ]);
    await mitools.run(finishRunLogCapture("run-a"));
    await mitools.run(finishRunLogCapture("run-b"));
  });

  it("drops the oldest lines beyond the per-run cap and counts them", async () => {
    startRunLogCapture("run-cap", "TaskA");
    await mitools.run(
      runWithLogCaptureEffect(
        "run-cap",
        Effect.forEach(
          Array.from({ length: MAX_LINES_PER_RUN + 100 }, (_, index) => index),
          (index) => logger.info(`line ${index}`),
          { discard: true },
        ),
      ),
    );
    const buffer = getActiveRunLogs("run-cap");
    expect(buffer?.lines).toHaveLength(MAX_LINES_PER_RUN);
    expect(buffer?.dropped).toBe(100);
    expect(buffer?.lines[0]?.msg).toBe("line 100");
    expect(buffer?.lines.at(-1)?.msg).toBe(`line ${MAX_LINES_PER_RUN + 99}`);
    await mitools.run(finishRunLogCapture("run-cap"));
  }, 15_000);

  it("truncates oversized lines", async () => {
    startRunLogCapture("run-long", "TaskA");
    await mitools.run(
      runWithLogCaptureEffect(
        "run-long",
        logger.info("x".repeat(MAX_LINE_LENGTH + 5_000)),
      ),
    );
    const line = getActiveRunLogs("run-long")?.lines[0];
    expect(line?.msg.length).toBe(MAX_LINE_LENGTH + 1);
    expect(line?.msg.endsWith("…")).toBe(true);
    await mitools.run(finishRunLogCapture("run-long"));
  });

  it("persists the buffer on finish, emits end, and clears the live buffer", async () => {
    const events: unknown[] = [];
    const unsubscribe = runLogBus.subscribe((event) => events.push(event));
    startRunLogCapture("run-3", "TaskC");
    await mitools.run(runWithLogCaptureEffect("run-3", logger.info("persisted line")));
    await mitools.run(finishRunLogCapture("run-3"));
    unsubscribe();
    expect(getActiveRunLogs("run-3")).toBeUndefined();
    const stored = await mitools.run(getRunLogs("run-3", logger));
    expect(stored?.taskName).toBe("TaskC");
    expect(stored?.lines).toHaveLength(1);
    expect(events.at(-1)).toEqual({ type: "end", runId: "run-3" });
  });

  it("reads legacy uncompressed log rows", async () => {
    await mitools.run(
      TaskRunLogEntity.upsert({
        runId: "run-legacy",
        taskName: "TaskC",
        lines: [{ t: 1, level: LogLevel.INFO, logger: "Test", msg: "old row" }],
        dropped: 0,
      }),
    );
    const stored = await mitools.run(getRunLogs("run-legacy", logger));
    expect(stored?.lines[0]?.msg).toBe("old row");
  });

  it("does not persist a row for runs that logged nothing", async () => {
    startRunLogCapture("run-empty", "TaskC");
    await mitools.run(finishRunLogCapture("run-empty"));
    await expect(mitools.run(getRunLogs("run-empty", logger))).resolves.toBeUndefined();
  });
});
