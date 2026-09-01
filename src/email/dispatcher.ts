import type { Logger } from "@micthiesen/mitools/logging";
import { Cause, Data, Effect, Fiber, Queue } from "effect";
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
  private readonly lifecycleSemaphore = Effect.unsafeMakeSemaphore(1);
  private triggerQueue: Queue.Queue<"trigger" | "stop"> | undefined;
  private supervisor: Fiber.RuntimeFiber<void, never> | undefined;

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
    this.triggerQueue?.unsafeOffer("trigger");
  }

  /** Start the owned trigger supervisor before push monitoring can emit. */
  public get startEffect(): Effect.Effect<void, unknown> {
    return this.lifecycleSemaphore.withPermits(1)(
      Effect.gen(this, function* () {
        if (this.supervisor) return;
        const queue = yield* Queue.bounded<"trigger" | "stop">(1);
        this.triggerQueue = queue;
        this.supervisor = yield* Effect.forkDaemon(this.superviseEffect(queue));
        yield* this.transport
          .startEffect(() => this.onMailEvent())
          .pipe(
            Effect.onError(() =>
              this.transport.stopEffect.pipe(
                Effect.ensuring(this.stopSupervisorEffect),
              ),
            ),
          );
      }),
    );
  }

  /** Stop notifications, drain the queued final poll, then join the supervisor. */
  public get stopEffect(): Effect.Effect<void, never> {
    return this.lifecycleSemaphore.withPermits(1)(
      this.transport.stopEffect.pipe(Effect.ensuring(this.stopSupervisorEffect)),
    );
  }

  private get stopSupervisorEffect(): Effect.Effect<void, never> {
    return Effect.gen(this, function* () {
      const queue = this.triggerQueue;
      const supervisor = this.supervisor;
      if (!queue || !supervisor) return;
      yield* Queue.offer(queue, "stop");
      yield* Fiber.join(supervisor);
      yield* Queue.shutdown(queue);
      this.triggerQueue = undefined;
      this.supervisor = undefined;
    });
  }

  private superviseEffect(
    queue: Queue.Queue<"trigger" | "stop">,
  ): Effect.Effect<void, never> {
    return Effect.gen(this, function* () {
      while (true) {
        const message = yield* Queue.take(queue);
        if (message === "stop") return;
        yield* Effect.catchAllCause(this.processPassEffect, (cause) =>
          Effect.sync(() => this.logger.error("Dispatcher error", Cause.pretty(cause))),
        );
      }
    });
  }

  /** Deterministic direct trigger for tests and manual polling. */
  public get onMailEventEffect(): Effect.Effect<void, never> {
    return this.processPassEffect.pipe(
      Effect.catchAll((error) =>
        Effect.sync(() => this.logger.error("Dispatcher error", error.message)),
      ),
    );
  }

  private get processPassEffect(): Effect.Effect<
    void,
    EmailHandlerError | PersistenceError
  > {
    return Effect.gen(this, function* () {
      const poll = yield* this.transport.pollNewEmailsEffect.pipe(
        Effect.mapError(
          (cause) => new EmailHandlerError({ handler: this.transport.name, cause }),
        ),
      );
      if (poll.emails.length > 0) yield* this.dispatchEffect(poll.emails);
      // Cursor advancement is valid only after every handler durably accepts the
      // batch. On failure, replay is intentional and downstream dedup is the gate.
      yield* Effect.try({
        try: poll.commit,
        catch: (cause) =>
          new EmailHandlerError({ handler: `${this.transport.name} cursor`, cause }),
      });
    });
  }

  private dispatchEffect(emails: FetchedEmail[]) {
    return Effect.gen(this, function* () {
      const results = yield* Effect.forEach(
        this.handlers,
        (handler) =>
          handler.handleEmailsEffect(emails).pipe(
            Effect.mapError(
              (cause) => new EmailHandlerError({ handler: handler.name, cause }),
            ),
            Effect.either,
          ),
        { concurrency: "unbounded" },
      );
      const failures = results.filter((result) => result._tag === "Left");
      for (const failure of failures) {
        this.logger.error(failure.left.message);
      }
      if (failures[0]) yield* Effect.fail(failures[0].left);
      yield* saveLastDispatchedAtEffect();
    });
  }
}
