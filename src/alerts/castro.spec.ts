import { Docstore } from "@micthiesen/mitools/docstore";
import { type LogNotification, LogLevel } from "@micthiesen/mitools/logging";
import { it } from "@effect/vitest";
import { Effect } from "effect";
import { describe, expect } from "vitest";
import { TaskRunEntity, type TaskRunData } from "../task-runs/persistence.js";
import { hasPersistentCastroFailure, persistentCastroFailureHook } from "./castro.js";

const hour = 60 * 60_000;
const run = (hours: number, status: TaskRunData["status"] = "error"): TaskRunData => ({
  runId: `castro-${hours}`,
  taskName: "CastroInboxCleanup",
  trigger: "schedule",
  startedAt: hours * hour,
  status,
});

describe("Castro persistent failure gate", () => {
  it.effect(
    "requires three consecutive failures spanning twelve hours, allowing jitter",
    () =>
      Effect.sync(() => {
        expect(hasPersistentCastroFailure([run(12)])).toBe(false);
        expect(hasPersistentCastroFailure([run(12), run(0)])).toBe(false);
        expect(hasPersistentCastroFailure([run(2), run(1), run(0)])).toBe(false);
        expect(hasPersistentCastroFailure([run(12), run(6), run(0)])).toBe(true);
        expect(hasPersistentCastroFailure([run(12), run(6), run(0.08)])).toBe(true);
        expect(hasPersistentCastroFailure([run(12), run(6, "success"), run(0)])).toBe(
          false,
        );
        expect(
          hasPersistentCastroFailure([run(18, "running"), run(12), run(6), run(0)]),
        ).toBe(false);
      }),
  );

  it.effect(
    "uses persisted history across fresh hooks for scheduled, manual and catch-up failures",
    () =>
      Effect.gen(function* () {
        const delivered: string[] = [];
        const notify = (title: string) => {
          // Construct afresh to model a service restart: no in-memory streak.
          const hook = persistentCastroFailureHook((notification: LogNotification) =>
            Effect.sync(() => {
              delivered.push(notification.title);
            }),
          );
          return hook({
            level: LogLevel.ERROR,
            loggerName: "Scheduler",
            title,
            body: "socket hang up",
          });
        };
        const scheduled = 'Error running task "CastroInboxCleanup"';
        yield* TaskRunEntity.upsert(run(0));
        yield* notify(scheduled);
        yield* TaskRunEntity.upsert(run(6));
        yield* notify(scheduled);
        expect(delivered).toEqual([]);
        yield* TaskRunEntity.upsert(run(12));
        yield* notify(scheduled);
        yield* notify('Manual run of "CastroInboxCleanup" failed');
        yield* notify('Catch-up run of "CastroInboxCleanup" failed');
        expect(delivered).toHaveLength(3);
        yield* TaskRunEntity.upsert(run(18, "success"));
        yield* TaskRunEntity.upsert(run(24));
        yield* notify(scheduled);
        expect(delivered).toHaveLength(3);
        yield* notify('Error running task "OtherTask"');
        expect(delivered).toHaveLength(4);
      }).pipe(Effect.provide(Docstore.layerMemory)),
  );
});
