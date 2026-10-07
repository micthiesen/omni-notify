import { Pushover } from "@micthiesen/mitools/pushover";
import { Effect, Layer } from "effect";
import { afterAll, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import { ResetSourceError } from "./source.js";
import { ResetAlertTask, type ResetSnapshot } from "./task.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

it("re-reads each poll, delivers once, and clears stale success on source failure", async () => {
  const now = Date.now();
  const snapshot: ResetSnapshot = {
    now,
    metadata: { feedItems: 1 },
    alerts: [
      {
        key: "task-test",
        title: "Reset reported",
        message: "Check Usage",
        url: "https://example.com",
        occurredAt: now,
      },
    ],
  };
  const read = vi.fn<() => Effect.Effect<ResetSnapshot, ResetSourceError>>(() =>
    Effect.succeed(snapshot),
  );
  const send = vi.fn(() => Effect.void);
  const pushover = Layer.succeed(
    Pushover,
    Pushover.of({ enabled: true, notify: send }),
  );
  const task = new ResetAlertTask(
    "ClaudeResets",
    "Claude Code Reset Alerts",
    "claude",
    "Reset Radar",
    Effect.suspend(read),
    runtime.logger,
  );
  const run = () => runtime.run(task.run.pipe(Effect.provide(pushover)));
  await run();
  expect(task.getLastRunSummary()).toContain("1 sent");
  await run();
  expect(task.getLastRunSummary()).toContain("1 already handled");
  expect(send).toHaveBeenCalledTimes(1);
  read.mockReturnValueOnce(
    Effect.fail(new ResetSourceError({ operation: "fetch", cause: "offline" })),
  );
  expect(
    await runtime.run(Effect.result(task.run).pipe(Effect.provide(pushover))),
  ).toMatchObject({ _tag: "Failure", failure: { _tag: "ResetSourceError" } });
  expect(task.getLastRunSummary()).toBeUndefined();
  expect(read).toHaveBeenCalledTimes(3);
  expect(send).toHaveBeenCalledTimes(1);
});
