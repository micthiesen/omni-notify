import type { Effect as EffectType } from "effect/Effect";
import { randomUUID } from "node:crypto";
import { Clock, Data, Deferred, Duration, Effect, Option, Schema } from "effect";

/** A poll this recent keeps the Mac online between long-polls. */
export const DEVICE_ONLINE_WINDOW_MS = 45_000;
export const DEVICE_POLL_HOLD = Duration.seconds(25);
export const DEVICE_PICKUP_TIMEOUT = Duration.seconds(30);

export type DeviceCommand =
  | "projects"
  | "list"
  | "status"
  | "read"
  | "result"
  | "wait"
  | "start"
  | "send"
  | "stop";

export interface DeviceJob {
  readonly id: string;
  readonly command: DeviceCommand;
  readonly args: Record<string, unknown>;
}

export type DeviceLinkStatus = {
  readonly configured: true;
  readonly online: boolean;
  readonly disabled: boolean;
  readonly host: string | null;
  readonly lastSeenAt: string | null;
  readonly pendingJobs: number;
};

export class DeviceLinkError extends Data.TaggedError("DeviceLinkError")<{
  readonly code: string;
  readonly detail: string;
  readonly retryable: boolean;
}> {
  public override get message(): string {
    return `${this.detail} (${this.code})`;
  }
}

const envelopeSchema = Schema.Union([
  Schema.Struct({
    v: Schema.Literal(1),
    ok: Schema.Literal(true),
    data: Schema.Record(Schema.String, Schema.Unknown),
  }),
  Schema.Struct({
    v: Schema.Literal(1),
    ok: Schema.Literal(false),
    error: Schema.Struct({
      code: Schema.String,
      message: Schema.String,
      retryable: Schema.optional(Schema.Boolean),
    }),
  }),
]);

/** What the Mac reported for one job: the client's envelope or a local failure. */
export type DeviceJobOutcome =
  | { readonly kind: "output"; readonly output: unknown }
  | {
      readonly kind: "error";
      readonly code: string;
      readonly message: string;
    };

interface Entry {
  readonly job: DeviceJob;
  state: "queued" | "delivered" | "withdrawn";
  readonly delivered: Deferred.Deferred<void>;
  readonly outcome: Deferred.Deferred<DeviceJobOutcome>;
}

/** Extra time for the Mac to spawn the command and post its own timeout result. */
const RESULT_SLACK = Duration.seconds(15);

/**
 * Relays bounded commands to the Mac's omni-link agent. The Mac long-polls for
 * jobs and posts results, so Omni never connects to the laptop. Claiming and
 * withdrawing a job are single synchronous state transitions: a job withdrawn
 * as "nothing ran" can never be delivered, and a delivered job whose result
 * never arrives is reported as an unknown outcome, never retried.
 */
export class DeviceLinkService {
  private readonly entries = new Map<string, Entry>();
  private wake = Deferred.makeUnsafe<void>();
  private releaseHeldPoll: Deferred.Deferred<void> | null = null;
  private activePolls = 0;
  private lastSeenAt: number | null = null;
  private disabled = false;
  private host: string | null = null;

  status(): EffectType<DeviceLinkStatus> {
    return Clock.currentTimeMillis.pipe(
      Effect.map((now) => ({
        configured: true as const,
        online: this.isOnline(now),
        disabled: this.disabled,
        host: this.host,
        lastSeenAt: this.lastSeenAt ? new Date(this.lastSeenAt).toISOString() : null,
        pendingJobs: this.entries.size,
      })),
    );
  }

  private isOnline(now: number): boolean {
    return (
      this.activePolls > 0 ||
      (this.lastSeenAt !== null && now - this.lastSeenAt < DEVICE_ONLINE_WINDOW_MS)
    );
  }

  private markSeen(report: { disabled: boolean; host: string | null }) {
    return Clock.currentTimeMillis.pipe(
      Effect.map((now) => {
        this.lastSeenAt = now;
        this.disabled = report.disabled;
        this.host = report.host;
      }),
    );
  }

  private claimQueued(): DeviceJob[] {
    const jobs: DeviceJob[] = [];
    for (const entry of this.entries.values()) {
      if (entry.state !== "queued") continue;
      entry.state = "delivered";
      Deferred.doneUnsafe(entry.delivered, Effect.void);
      jobs.push(entry.job);
    }
    return jobs;
  }

  /**
   * Hold one long-poll open and hand over every queued job. A newer poll
   * releases an older one, so a dead connection left by an agent restart stops
   * claiming jobs. Claiming cannot be interrupted once it begins.
   */
  poll(report: { disabled: boolean; host: string | null }): EffectType<DeviceJob[]> {
    return Effect.uninterruptibleMask((restore) =>
      Effect.gen({ self: this }, function* () {
        yield* this.markSeen(report);
        if (this.releaseHeldPoll)
          Deferred.doneUnsafe(this.releaseHeldPoll, Effect.void);
        const released = Deferred.makeUnsafe<void>();
        this.releaseHeldPoll = released;
        const hold = Effect.sleep(DEVICE_POLL_HOLD).pipe(
          Effect.raceFirst(Deferred.await(released)),
        );
        if (report.disabled) {
          yield* restore(hold);
          return [];
        }
        this.activePolls += 1;
        const jobs = yield* Effect.gen({ self: this }, function* () {
          const deadline =
            (yield* Clock.currentTimeMillis) + Duration.toMillis(DEVICE_POLL_HOLD);
          while (true) {
            const claimed = this.claimQueued();
            if (claimed.length > 0) return claimed;
            const remaining = deadline - (yield* Clock.currentTimeMillis);
            if (remaining <= 0 || Deferred.isDoneUnsafe(released)) return [];
            const wake = this.wake;
            yield* restore(
              Deferred.await(wake).pipe(
                Effect.raceFirst(Deferred.await(released)),
                Effect.timeoutOption(Duration.millis(remaining)),
              ),
            );
          }
        }).pipe(Effect.ensuring(Effect.sync(() => (this.activePolls -= 1))));
        if (this.releaseHeldPoll === released) this.releaseHeldPoll = null;
        yield* this.markSeen(report);
        return jobs;
      }),
    );
  }

  /** Accept a result for a delivered job; false means unknown or withdrawn. */
  complete(id: string, outcome: DeviceJobOutcome): EffectType<boolean> {
    return Effect.sync(() => {
      const entry = this.entries.get(id);
      if (!entry || entry.state !== "delivered") return false;
      return Deferred.doneUnsafe(entry.outcome, Effect.succeed(outcome));
    });
  }

  /** Run one command on the Mac and return the client's `data` payload. */
  execute(
    command: DeviceCommand,
    args: Record<string, unknown>,
    resultTimeout: Duration.Duration,
  ): EffectType<Record<string, unknown>, DeviceLinkError> {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      if (!this.isOnline(now)) {
        return yield* new DeviceLinkError({
          code: "offline",
          detail: `The Claude Code host is offline (last seen ${
            this.lastSeenAt ? new Date(this.lastSeenAt).toISOString() : "never"
          }); nothing ran`,
          retryable: true,
        });
      }
      if (this.disabled) {
        return yield* new DeviceLinkError({
          code: "disabled",
          detail:
            "Session control is disabled on the Claude Code host (kill switch); nothing ran",
          retryable: false,
        });
      }
      const entry: Entry = {
        job: { id: yield* Effect.sync(() => randomUUID()), command, args },
        state: "queued",
        delivered: yield* Deferred.make<void>(),
        outcome: yield* Deferred.make<DeviceJobOutcome>(),
      };
      const withdrawIfQueued = Effect.sync(() => {
        if (entry.state !== "queued") return false;
        entry.state = "withdrawn";
        return true;
      });
      const outcome = yield* Effect.gen({ self: this }, function* () {
        this.entries.set(entry.job.id, entry);
        const wake = this.wake;
        this.wake = Deferred.makeUnsafe<void>();
        Deferred.doneUnsafe(wake, Effect.void);
        yield* Deferred.await(entry.delivered).pipe(
          Effect.timeoutOption(DEVICE_PICKUP_TIMEOUT),
        );
        if (yield* withdrawIfQueued) {
          return yield* new DeviceLinkError({
            code: "not_picked_up",
            detail:
              "The Claude Code host did not pick up the request in time; nothing ran",
            retryable: true,
          });
        }
        const reported = yield* Deferred.await(entry.outcome).pipe(
          Effect.timeoutOption(Duration.sum(resultTimeout, RESULT_SLACK)),
        );
        if (Option.isNone(reported)) {
          return yield* new DeviceLinkError({
            code: "outcome_unknown",
            detail:
              "The Claude Code host accepted the request but sent no result in time; the outcome is unknown. Check the session before retrying",
            retryable: false,
          });
        }
        return reported.value;
      }).pipe(
        Effect.ensuring(
          withdrawIfQueued.pipe(
            Effect.andThen(Effect.sync(() => this.entries.delete(entry.job.id))),
          ),
        ),
      );
      return yield* decodeOutcome(outcome);
    });
  }
}

function decodeOutcome(
  outcome: DeviceJobOutcome,
): EffectType<Record<string, unknown>, DeviceLinkError> {
  if (outcome.kind === "error") {
    return Effect.fail(
      new DeviceLinkError({
        code: outcome.code,
        detail: outcome.message,
        retryable: false,
      }),
    );
  }
  return Schema.decodeUnknownEffect(envelopeSchema)(outcome.output).pipe(
    Effect.mapError(
      () =>
        new DeviceLinkError({
          code: "bad_output",
          detail: "The Claude Code host returned malformed output",
          retryable: false,
        }),
    ),
    Effect.flatMap((envelope) =>
      envelope.ok
        ? Effect.succeed({ ...envelope.data })
        : Effect.fail(
            new DeviceLinkError({
              code: envelope.error.code,
              detail: envelope.error.message,
              retryable: envelope.error.retryable ?? false,
            }),
          ),
    ),
  );
}
