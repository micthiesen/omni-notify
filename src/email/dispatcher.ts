import type { Logger } from "@micthiesen/mitools/logging";
import { Cause, Data, Effect, Exit, Fiber, Queue, Scope, Semaphore } from "effect";
import type { PersistenceError } from "../effect/errors.js";
import { saveLastDispatchedAtEffect } from "./persistence.js";
import type { EmailHandler, EmailTransport, FetchedEmail } from "./types.js";

/**
 * Transport-agnostic fan-out: on every mail event, polls the transport for
 * new emails and dispatches them to all registered handlers. Events that
 * land mid-processing receive a ticket and force another pass (never dropped).
 */
export class EmailHandlerError extends Data.TaggedError("EmailHandlerError")<{
  readonly handler: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `Handler "${this.handler}" failed: ${detail}`;
  }
}

export class EmailDispatcher {
  private readonly handlers: EmailHandler[] = [];
  private readonly lifecycleSemaphore = Semaphore.makeUnsafe(1);
  private triggerQueue: Queue.Queue<"trigger" | "stop"> | undefined;
  private supervisor: Fiber.Fiber<void, never> | undefined;
  private supervisorScope: Scope.Closeable | undefined;

  constructor(
    private readonly transport: EmailTransport,
    private readonly logger: Logger,
  ) {}

  register(handler: EmailHandler): void {
    this.handlers.push(handler);
  }

  get handlerCount(): number {
    return this.handlers.length;
  }

  onMailEvent(): void {
    // This callback is a synchronous library boundary. `unsafeOffer` does not
    // create an unowned fiber. The capacity-one queue coalesces notification
    // bursts while preserving one final poll after an active pass.
    if (this.triggerQueue) Queue.offerUnsafe(this.triggerQueue, "trigger");
  }

  /** Start the owned trigger supervisor before push monitoring can emit. */
  public get startEffect(): Effect.Effect<void, EmailHandlerError> {
    return this.lifecycleSemaphore.withPermits(1)(
      Effect.uninterruptibleMask((restore) =>
        Effect.gen({ self: this }, function* () {
          if (this.supervisor) return;
          const queue = yield* Queue.bounded<"trigger" | "stop">(1);
          const scope = yield* Scope.make();
          const supervisor = yield* this.superviseEffect(queue).pipe(
            Effect.forkScoped,
            Effect.provideService(Scope.Scope, scope),
          );
          this.triggerQueue = queue;
          this.supervisor = supervisor;
          this.supervisorScope = scope;
          yield* restore(this.transport.startEffect(() => this.onMailEvent())).pipe(
            Effect.mapError(
              (cause) => new EmailHandlerError({ handler: this.transport.name, cause }),
            ),
            Effect.onExit((exit) =>
              Exit.isSuccess(exit)
                ? Effect.void
                : this.transport.stopEffect.pipe(
                    Effect.andThen(this.stopSupervisorEffect),
                  ),
            ),
          );
        }),
      ),
    );
  }

  /** Stop notifications, drain the queued final poll, then join the supervisor. */
  public get stopEffect(): Effect.Effect<void, never> {
    return this.lifecycleSemaphore.withPermits(1)(
      this.transport.stopEffect.pipe(Effect.ensuring(this.stopSupervisorEffect)),
    );
  }

  private get stopSupervisorEffect(): Effect.Effect<void, never> {
    return Effect.gen({ self: this }, function* () {
      const queue = this.triggerQueue;
      const supervisor = this.supervisor;
      const scope = this.supervisorScope;
      this.triggerQueue = undefined;
      this.supervisor = undefined;
      this.supervisorScope = undefined;
      if (!queue || !supervisor) {
        if (scope) yield* Scope.close(scope, Exit.succeed(undefined));
        return;
      }
      yield* Queue.offer(queue, "stop");
      yield* Fiber.join(supervisor);
      yield* Queue.shutdown(queue);
      if (scope) yield* Scope.close(scope, Exit.succeed(undefined));
    });
  }

  private superviseEffect(
    queue: Queue.Queue<"trigger" | "stop">,
  ): Effect.Effect<void, never> {
    return Effect.gen({ self: this }, function* () {
      while (true) {
        const message = yield* Queue.take(queue);
        if (message === "stop") return;
        yield* Effect.catchCause(this.processPassEffect, (cause) =>
          Effect.sync(() => this.logger.error("Dispatcher error", Cause.pretty(cause))),
        );
      }
    });
  }

  /** Deterministic direct trigger for tests and manual polling. */
  public get onMailEventEffect(): Effect.Effect<void, never> {
    return this.processPassEffect.pipe(
      Effect.catch((error) =>
        Effect.sync(() => this.logger.error("Dispatcher error", error.message)),
      ),
    );
  }

  private get processPassEffect(): Effect.Effect<
    void,
    EmailHandlerError | PersistenceError
  > {
    return Effect.gen({ self: this }, function* () {
      const poll = yield* this.transport.pollNewEmailsEffect.pipe(
        Effect.mapError(
          (cause) => new EmailHandlerError({ handler: this.transport.name, cause }),
        ),
      );
      if (poll.emails.length > 0) yield* this.dispatchEffect(poll.emails);
      // Cursor advancement is valid only after every handler durably accepts the
      // batch. On failure, replay is intentional and downstream dedup is the gate.
      yield* poll.commit.pipe(
        Effect.mapError(
          (cause) =>
            new EmailHandlerError({ handler: `${this.transport.name} cursor`, cause }),
        ),
      );
      if (poll.emails.length > 0) yield* saveLastDispatchedAtEffect();
    });
  }

  private dispatchEffect(emails: FetchedEmail[]) {
    return Effect.gen({ self: this }, function* () {
      const results = yield* Effect.forEach(
        this.handlers,
        (handler) =>
          handler.handleEmailsEffect(emails).pipe(
            Effect.mapError(
              (cause) => new EmailHandlerError({ handler: handler.name, cause }),
            ),
            Effect.result,
          ),
        { concurrency: "unbounded" },
      );
      const failures = results.filter((result) => result._tag === "Failure");
      for (const failure of failures) {
        this.logger.error(failure.failure.message);
      }
      if (failures[0]) yield* Effect.fail(failures[0].failure);
    });
  }
}
