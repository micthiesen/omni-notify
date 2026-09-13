import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { Effect } from "effect";
import {
  acquireIssue,
  listPending,
  OBSERVER_REPAIR_LEASE_MS,
  ObserverRepairStateEntity,
  saveIssue,
} from "./persistence.js";

const NOW = 1_800_000_000_000;

layer(Docstore.layerMemory)("Observer repair persistence", (it) => {
  it.effect("allows only one worker to reserve an issue", () =>
    Effect.gen(function* () {
      yield* ObserverRepairStateEntity.deleteAll();
      const states = yield* Effect.all(
        Array.from({ length: 10 }, (_, index) =>
          acquireIssue(42, "revision-1", `worker-${index}`, NOW),
        ),
        { concurrency: "unbounded" },
      );
      expect(states.filter((state) => state !== undefined)).toHaveLength(1);
    }),
  );

  it.effect(
    "permits a new owner after lease expiry and marks interrupted execution unhandled",
    () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const first = yield* acquireIssue(7, "r1", "first", NOW);
        yield* saveIssue({ ...first!, phase: "executing" }, "first", NOW + 1);
        const resumed = yield* acquireIssue(
          7,
          "r1",
          "second",
          NOW + OBSERVER_REPAIR_LEASE_MS,
        );
        expect(resumed?.phase).toBe("unhandled");
        expect(resumed?.outcome).toBe("unhandled");
        expect(resumed?.lease?.owner).toBe("second");
      }),
  );

  it.effect(
    "skips completed revisions and resets only completed state for a new revision",
    () =>
      Effect.gen(function* () {
        yield* ObserverRepairStateEntity.deleteAll();
        const first = yield* acquireIssue(9, "r1", "worker", NOW);
        yield* saveIssue(
          { ...first!, phase: "done", outcome: "repaired" },
          "worker",
          NOW + 1,
        );
        expect(yield* acquireIssue(9, "r1", "other", NOW + 2)).toBeUndefined();
        const next = yield* acquireIssue(9, "r2", "other", NOW + 2);
        expect(next?.phase).toBe("reserved");
        expect(next?.revision).toBe("r2");
        const active = yield* acquireIssue(10, "r1", "worker", NOW);
        expect(yield* acquireIssue(10, "r2", "other", NOW + 1)).toBeUndefined();
        expect(active?.revision).toBe("r1");
      }),
  );

  it.effect("lists at most 100 unfinished states", () =>
    Effect.gen(function* () {
      yield* ObserverRepairStateEntity.deleteAll();
      for (let issueId = 1; issueId <= 101; issueId += 1) {
        yield* acquireIssue(issueId, "r1", "worker", NOW);
      }
      const first = yield* acquireIssue(1000, "r1", "worker", NOW);
      yield* saveIssue({ ...first!, phase: "done" }, "worker", NOW + 1);
      expect(yield* listPending()).toHaveLength(100);
    }),
  );
});
