import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { runLogBus } from "./events.js";
import {
  finishRunLogCapture,
  getActiveRunLogs,
  installLogCapture,
  runWithLogCapture,
  startRunLogCapture,
} from "./logCapture.js";
import { getRunLogs, TaskRunLogEntity } from "./persistence.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "logcapture.spec.db",
  },
});

const logger = new Logger("Test");

beforeAll(() => {
  installLogCapture();
  // Keep captured console output out of the test run.
  vi.spyOn(console, "debug").mockImplementation(() => {});
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  TaskRunLogEntity.deleteAll();
});

describe("log capture", () => {
  it("attributes lines logged inside the run context, including sub-loggers", async () => {
    startRunLogCapture("run-1", "TaskA");
    await runWithLogCapture("run-1", async () => {
      logger.info("hello");
      await Promise.resolve();
      logger.extend("Sub").warn("careful", { code: 7 });
    });

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
    finishRunLogCapture("run-1");
  });

  it("captures DEBUG lines even when LOG_LEVEL is info", async () => {
    startRunLogCapture("run-debug", "TaskA");
    await runWithLogCapture("run-debug", async () => {
      logger.debug("below console threshold");
    });

    expect(getActiveRunLogs("run-debug")?.lines).toHaveLength(1);
    expect(console.debug).not.toHaveBeenCalled();
    finishRunLogCapture("run-debug");
  });

  it("ignores lines logged outside any run context", () => {
    startRunLogCapture("run-2", "TaskA");
    logger.info("ambient log");
    expect(getActiveRunLogs("run-2")?.lines).toHaveLength(0);
    finishRunLogCapture("run-2");
  });

  it("keeps concurrent runs separate", async () => {
    startRunLogCapture("run-a", "TaskA");
    startRunLogCapture("run-b", "TaskB");
    await Promise.all([
      runWithLogCapture("run-a", async () => {
        logger.info("from A");
        await new Promise((resolve) => setTimeout(resolve, 5));
        logger.info("from A again");
      }),
      runWithLogCapture("run-b", async () => {
        await new Promise((resolve) => setTimeout(resolve, 2));
        logger.info("from B");
      }),
    ]);

    expect(getActiveRunLogs("run-a")?.lines.map((l) => l.msg)).toEqual([
      "from A",
      "from A again",
    ]);
    expect(getActiveRunLogs("run-b")?.lines.map((l) => l.msg)).toEqual(["from B"]);
    finishRunLogCapture("run-a");
    finishRunLogCapture("run-b");
  });

  it("drops the oldest lines beyond the per-run cap and counts them", async () => {
    startRunLogCapture("run-cap", "TaskA");
    await runWithLogCapture("run-cap", async () => {
      for (let i = 0; i < 2100; i++) logger.info(`line ${i}`);
    });

    const buffer = getActiveRunLogs("run-cap");
    expect(buffer?.lines).toHaveLength(2000);
    expect(buffer?.dropped).toBe(100);
    expect(buffer?.lines[0]?.msg).toBe("line 100");
    expect(buffer?.lines.at(-1)?.msg).toBe("line 2099");
    finishRunLogCapture("run-cap");
  });

  it("truncates oversized lines", async () => {
    startRunLogCapture("run-long", "TaskA");
    await runWithLogCapture("run-long", async () => {
      logger.info("x".repeat(10_000));
    });

    const line = getActiveRunLogs("run-long")?.lines[0];
    expect(line?.msg.length).toBe(4097); // 4096 chars + ellipsis
    expect(line?.msg.endsWith("…")).toBe(true);
    finishRunLogCapture("run-long");
  });

  it("persists the buffer on finish, emits end, and clears the live buffer", async () => {
    const events: unknown[] = [];
    const unsubscribe = runLogBus.subscribe((event) => events.push(event));
    startRunLogCapture("run-3", "TaskC");
    await runWithLogCapture("run-3", async () => {
      logger.info("persisted line");
    });
    finishRunLogCapture("run-3");
    unsubscribe();

    expect(getActiveRunLogs("run-3")).toBeUndefined();
    const stored = getRunLogs("run-3");
    expect(stored?.taskName).toBe("TaskC");
    expect(stored?.lines).toHaveLength(1);
    expect(events.at(-1)).toEqual({ type: "end", runId: "run-3" });
  });

  it("does not persist a row for runs that logged nothing", () => {
    startRunLogCapture("run-empty", "TaskC");
    finishRunLogCapture("run-empty");
    expect(getRunLogs("run-empty")).toBeUndefined();
  });
});
