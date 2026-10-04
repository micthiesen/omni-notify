import { describe, expect, it } from "@effect/vitest";
import { Duration, Effect, Fiber, Result } from "effect";
import { TestClock } from "effect/testing";
import { DeviceLinkError, DeviceLinkService } from "./service.js";

const online = { disabled: false, host: "MaxBook" };

/** Mark the Mac as seen with an immediately answered poll. */
const checkIn = (link: DeviceLinkService, report = online) =>
  Effect.gen(function* () {
    const poll = yield* Effect.forkChild(link.poll(report));
    yield* TestClock.adjust(Duration.seconds(25));
    return yield* Fiber.join(poll);
  });

const failure = <A>(effect: Effect.Effect<A, DeviceLinkError>) =>
  Effect.result(effect).pipe(
    Effect.map((result) => (Result.isFailure(result) ? result.failure : undefined)),
  );

describe("DeviceLinkService", () => {
  it.effect("refuses commands while the Mac has never checked in", () =>
    Effect.gen(function* () {
      const link = new DeviceLinkService();
      const error = yield* failure(link.execute("list", {}, Duration.seconds(5)));
      expect(error?.code).toBe("offline");
      expect((yield* link.status()).online).toBe(false);
    }),
  );

  it.effect("delivers a job to a waiting poll and returns the envelope data", () =>
    Effect.gen(function* () {
      const link = new DeviceLinkService();
      const poll = yield* Effect.forkChild(link.poll(online));
      const call = yield* Effect.forkChild(
        link.execute("status", { session: "abc123" }, Duration.seconds(5)),
      );
      const jobs = yield* Fiber.join(poll);
      expect(jobs).toEqual([
        { id: expect.any(String), command: "status", args: { session: "abc123" } },
      ]);
      const accepted = yield* link.complete(jobs[0]!.id, {
        kind: "output",
        output: { v: 1, ok: true, data: { session_id: "abc123-full" } },
      });
      expect(accepted).toBe(true);
      expect(yield* Fiber.join(call)).toEqual({ session_id: "abc123-full" });
      expect((yield* link.status()).pendingJobs).toBe(0);
      expect(
        yield* link.complete(jobs[0]!.id, { kind: "error", code: "x", message: "y" }),
      ).toBe(false);
    }),
  );

  it.effect(
    "maps client error envelopes and local agent errors to typed failures",
    () =>
      Effect.gen(function* () {
        const link = new DeviceLinkService();
        for (const [outcome, code] of [
          [
            {
              kind: "output" as const,
              output: {
                v: 1,
                ok: false,
                error: { code: "busy", message: "mid-turn", retryable: true },
              },
            },
            "busy",
          ],
          [
            { kind: "error" as const, code: "timeout", message: "took too long" },
            "timeout",
          ],
          [{ kind: "output" as const, output: { nope: true } }, "bad_output"],
        ] as const) {
          const poll = yield* Effect.forkChild(link.poll(online));
          const call = yield* Effect.forkChild(
            failure(link.execute("send", { session: "abc123" }, Duration.seconds(5))),
          );
          const [job] = yield* Fiber.join(poll);
          yield* link.complete(job!.id, outcome);
          const error = yield* Fiber.join(call);
          expect(error).toBeInstanceOf(DeviceLinkError);
          expect(error?.code).toBe(code);
        }
      }),
  );

  it.effect("withdraws a job the Mac never picks up so it cannot run later", () =>
    Effect.gen(function* () {
      const link = new DeviceLinkService();
      yield* checkIn(link);
      const call = yield* Effect.forkChild(
        failure(link.execute("stop", { session: "abc123" }, Duration.seconds(5))),
      );
      yield* TestClock.adjust(Duration.seconds(31));
      expect((yield* Fiber.join(call))?.code).toBe("not_picked_up");
      const poll = yield* Effect.forkChild(link.poll(online));
      yield* TestClock.adjust(Duration.seconds(25));
      expect(yield* Fiber.join(poll)).toEqual([]);
    }),
  );

  it.effect("reports an unknown outcome when a delivered job never answers", () =>
    Effect.gen(function* () {
      const link = new DeviceLinkService();
      const poll = yield* Effect.forkChild(link.poll(online));
      const call = yield* Effect.forkChild(
        failure(link.execute("send", { session: "abc123" }, Duration.seconds(10))),
      );
      const [job] = yield* Fiber.join(poll);
      yield* TestClock.adjust(Duration.seconds(26));
      expect((yield* Fiber.join(call))?.code).toBe("outcome_unknown");
      expect(yield* link.complete(job!.id, { kind: "output", output: {} })).toBe(false);
    }),
  );

  it.effect("honors the Mac's kill switch and the online window", () =>
    Effect.gen(function* () {
      const link = new DeviceLinkService();
      expect(yield* checkIn(link, { disabled: true, host: "MaxBook" })).toEqual([]);
      const disabled = yield* failure(link.execute("list", {}, Duration.seconds(5)));
      expect(disabled?.code).toBe("disabled");

      yield* checkIn(link);
      expect((yield* link.status()).online).toBe(true);
      yield* TestClock.adjust(Duration.seconds(46));
      const status = yield* link.status();
      expect(status).toMatchObject({ online: false, disabled: false, host: "MaxBook" });
      expect(
        (yield* failure(link.execute("list", {}, Duration.seconds(5))))?.code,
      ).toBe("offline");
    }),
  );

  it.effect("releases an older held poll when a newer one arrives", () =>
    Effect.gen(function* () {
      const link = new DeviceLinkService();
      const stale = yield* Effect.forkChild(link.poll(online));
      yield* TestClock.adjust(Duration.seconds(1));
      const fresh = yield* Effect.forkChild(link.poll(online));
      expect(yield* Fiber.join(stale)).toEqual([]);
      const call = yield* Effect.forkChild(
        link.execute("projects", {}, Duration.seconds(5)),
      );
      const [job] = yield* Fiber.join(fresh);
      expect(job?.command).toBe("projects");
      yield* link.complete(job!.id, {
        kind: "output",
        output: { v: 1, ok: true, data: { projects: [] } },
      });
      expect(yield* Fiber.join(call)).toEqual({ projects: [] });
    }),
  );

  it.effect(
    "keeps waiting for a result past the command timeout by a slack margin",
    () =>
      Effect.gen(function* () {
        const link = new DeviceLinkService();
        const poll = yield* Effect.forkChild(link.poll(online));
        const call = yield* Effect.forkChild(
          link.execute("start", { project: "omni-notify" }, Duration.seconds(10)),
        );
        const [job] = yield* Fiber.join(poll);
        yield* TestClock.adjust(Duration.seconds(20));
        yield* link.complete(job!.id, {
          kind: "error",
          code: "timeout",
          message: "claude-for-dot did not finish",
        });
        const error = yield* failure(Fiber.join(call));
        expect(error?.code).toBe("timeout");
      }),
  );
});
