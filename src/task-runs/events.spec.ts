import { expect, it, vi } from "vitest";
import { taskRunBus } from "./events.js";

it("isolates a failing task-run subscriber", () => {
  const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
  const received: string[] = [];
  const unsubscribeBroken = taskRunBus.subscribe(() => {
    throw new Error("broken dashboard");
  });
  const unsubscribeHealthy = taskRunBus.subscribe((event) => {
    received.push(event.taskName);
  });

  taskRunBus.emit({ type: "run-started", taskName: "HealthyTask" });

  expect(received).toEqual(["HealthyTask"]);
  expect(consoleError).toHaveBeenCalledWith(
    "Task event subscriber failed",
    expect.any(Error),
  );

  unsubscribeBroken();
  unsubscribeHealthy();
  consoleError.mockRestore();
});
