import type { Logger } from "@micthiesen/mitools/logging";
import { EventSource } from "eventsource";
import { Data, Effect, Fiber, Random, Schema, Scope } from "effect";
import type { JmapContext } from "./client.js";

// Inactivity timeout modeled after Fastmail's own Overture client (6 min default):
// https://github.com/fastmail/overture/blob/master/source/io/EventSource.js
//
// The `eventsource` npm library has NO built-in staleness detection. If a TCP
// connection silently dies (half-open), the library will sit idle forever. This
// timer resets on every received event (state, ping, open, error). If nothing
// arrives within the window, we assume the connection is dead and force reconnect.
//
// Why 6 minutes: Overture uses 360,000ms. We request `ping=60` (60s server pings)
// per RFC 8620 §7.3, so 6 min tolerates several missed pings before acting.
// See: https://www.rfc-editor.org/rfc/rfc8620#section-7.3
const INACTIVITY_TIMEOUT_MS = 6 * 60_000;

// Exponential backoff modeled after Overture's reconnection strategy:
// - First attempt: random jitter 0-3s to avoid thundering herd
// - Subsequent: double previous delay, capped at 5 min
// - Reset to INITIAL_BACKOFF_MS on successful connection (open event)
// https://github.com/fastmail/overture/blob/master/source/io/EventSource.js
const INITIAL_BACKOFF_MS = 1_000;
const MAX_BACKOFF_MS = 5 * 60_000;

const StateEventSchema = Schema.Struct({
  changed: Schema.Record({
    key: Schema.String,
    value: Schema.Record({ key: Schema.String, value: Schema.String }),
  }),
});

export class EventSourceSetupError extends Data.TaggedError("EventSourceSetupError")<{
  readonly cause: unknown;
}> {
  public override get message(): string {
    return this.cause instanceof Error ? this.cause.message : String(this.cause);
  }
}

/**
 * Resilient SSE connection to Fastmail's JMAP event source for real-time email
 * notifications. Wraps the `eventsource` npm library with additional reliability:
 *
 * - **Inactivity timeout** (6 min): The `eventsource` library auto-reconnects on
 *   connection drops, but cannot detect silently-dead TCP connections. This timer
 *   resets on every event and forces a reconnect if the connection goes quiet.
 *   Pattern borrowed from Fastmail's Overture client.
 *
 * - **Exponential backoff with jitter**: The `eventsource` library uses a fixed 3s
 *   retry. We override reconnection with backoff (1s -> 5min cap) to avoid
 *   hammering Fastmail during outages.
 *
 * - **Ping listener**: RFC 8620 §7.3 defines `event: ping` keepalives. We request
 *   them via `ping=60` in the URL and use them to reset the inactivity timer.
 *   Even if the server doesn't send pings, state events still reset the timer.
 *
 * URL params (per RFC 8620 §7.3):
 * - `types=Email`: Only subscribe to Email state changes
 * - `closeafter=no`: Keep connection open (vs "state" which closes after first push)
 * - `ping=60`: Request 60s keepalive pings (server may enforce a 30s minimum)
 */
class JmapEventSource {
  private es: EventSource | null = null;
  private inactivityFiber: Fiber.RuntimeFiber<void, never> | null = null;
  private reconnectFiber: Fiber.RuntimeFiber<void, never> | null = null;
  private backoffMs = 0;
  private connected = false;
  private closed = false;

  constructor(
    private readonly url: string,
    private readonly bearerToken: string,
    private readonly onEmailStateChange: () => void,
    private readonly logger: Logger,
  ) {}

  readonly connectEffect: Effect.Effect<void, EventSourceSetupError> = Effect.try({
    try: () => {
      const eventSource = new EventSource(this.url, {
        fetch: (input, init) => {
          const headers = (init?.headers ?? {}) as Record<string, string>;
          headers.Authorization = this.bearerToken;
          return globalThis.fetch(input, { ...init, headers });
        },
      });
      this.es = eventSource;

      eventSource.addEventListener("open", () => {
        this.backoffMs = INITIAL_BACKOFF_MS;
        if (!this.connected) {
          this.logger.info("EventSource connected");
          this.connected = true;
        }
        this.resetInactivityTimer();
        // Catch-up drain: state pushes that fired while we were disconnected are
        // gone forever, so treat every (re)connect as a potential missed change
        // and let the dispatcher diff against its saved state immediately.
        this.onEmailStateChange();
      });

      // RFC 8620 §7.3: `event: state` with JSON payload
      // { changed: { [accountId]: { [dataType]: newState } } }
      eventSource.addEventListener("state", (event) => {
        this.resetInactivityTimer();
        this.handleStateEvent(event);
      });

      // RFC 8620 §7.3: `event: ping` with { interval: <ms> } payload.
      // Serves as a keepalive heartbeat to prove the connection is alive.
      eventSource.addEventListener("ping", () => {
        this.resetInactivityTimer();
      });

      // Fastmail closes the SSE connection after each state push. The `eventsource`
      // library fires an error event for these disconnects and auto-reconnects.
      // We reset the inactivity timer here since even error events prove liveness.
      eventSource.addEventListener("error", (event) => {
        this.resetInactivityTimer();
        this.handleErrorEvent(event);
      });

      this.resetInactivityTimer();
    },
    catch: (cause) => new EventSourceSetupError({ cause }),
  }).pipe(Effect.tapError(() => this.rollbackConnectionEffect));

  /**
   * Undo a failed connection attempt without permanently shutting down the
   * resource. Reconnect setup failures are transient; only closeEffect may set
   * `closed` and prevent later attempts.
   */
  private readonly rollbackConnectionEffect: Effect.Effect<void, never> = Effect.gen(
    this,
    function* () {
      if (this.inactivityFiber) yield* Fiber.interrupt(this.inactivityFiber);
      this.inactivityFiber = null;
      this.es?.close();
      this.es = null;
    },
  );

  readonly closeEffect: Effect.Effect<void, never> = Effect.gen(this, function* () {
    this.closed = true;
    if (this.inactivityFiber) yield* Fiber.interrupt(this.inactivityFiber);
    if (this.reconnectFiber) yield* Fiber.interrupt(this.reconnectFiber);
    this.inactivityFiber = null;
    this.reconnectFiber = null;
    this.logger.info("Closing EventSource connection");
    this.es?.close();
    this.es = null;
  });

  private handleStateEvent(event: MessageEvent): void {
    try {
      const data = Schema.decodeUnknownSync(StateEventSchema)(JSON.parse(event.data));

      const hasEmailChange = Object.values(data.changed).some(
        (changes) => "Email" in changes,
      );
      if (hasEmailChange) {
        this.logger.debug("Email state change detected");
        this.onEmailStateChange();
      }
    } catch (error) {
      this.logger.error("Failed to parse state change event", (error as Error).message);
    }
  }

  private handleErrorEvent(event: Event): void {
    const code = (event as { code?: number }).code;
    const message = (event as { message?: string }).message;

    // Auth failures are permanent; no point retrying.
    if (code === 401 || code === 403) {
      this.logger.error(
        "EventSource auth error, closing connection",
        `Code ${code}: ${message}`,
      );
      Effect.runFork(this.closeEffect);
      return;
    }

    // Connection-close errors are expected (see Fastmail behavior note on connect()).
    // Only log when there's an actual error message.
    if (message) {
      this.logger.warn(`EventSource error: ${message}`);
    }
  }

  private resetInactivityTimer(): void {
    if (this.closed) return;
    const previous = this.inactivityFiber;
    this.inactivityFiber = Effect.runFork(
      Effect.gen(this, function* () {
        if (previous) yield* Fiber.interrupt(previous);
        yield* Effect.sleep(INACTIVITY_TIMEOUT_MS);
        if (this.closed) return;
        this.logger.warn("EventSource inactivity timeout, forcing reconnect");
        yield* this.reconnectEffect;
      }).pipe(Effect.catchAllCause(() => Effect.void)),
    );
  }

  private readonly reconnectEffect: Effect.Effect<void, never> = Effect.gen(
    this,
    function* () {
      this.es?.close();
      this.es = null;
      if (this.closed) return;
      if (this.reconnectFiber) yield* Fiber.interrupt(this.reconnectFiber);
      let fiber!: Fiber.RuntimeFiber<void, never>;
      fiber = yield* this.reconnectLoopEffect.pipe(
        Effect.ensuring(
          Effect.sync(() => {
            if (this.reconnectFiber === fiber) this.reconnectFiber = null;
          }),
        ),
        Effect.forkDaemon,
      );
      this.reconnectFiber = fiber;
    },
  );

  private readonly reconnectLoopEffect: Effect.Effect<void, never> = Effect.gen(
    this,
    function* () {
      const delay =
        this.backoffMs === 0
          ? yield* Random.nextIntBetween(0, 3_001)
          : Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
      this.backoffMs = delay;
      this.logger.debug(`EventSource reconnecting in ${delay}ms`);
      yield* Effect.sleep(delay);
      if (this.closed) return;
      yield* this.connectEffect.pipe(
        Effect.catchAll((error) =>
          Effect.sync(() =>
            this.logger.warn(`EventSource reconnect failed: ${error.message}`),
          ).pipe(Effect.zipRight(Effect.suspend(() => this.reconnectLoopEffect))),
        ),
      );
    },
  );
}

/**
 * Acquire a resilient JMAP EventSource connection in the current Scope.
 * Closing the Scope closes the connection and cancels all timers.
 */
export function createEventSourceEffect(
  ctx: JmapContext,
  onEmailStateChange: () => void,
  logger: Logger,
): Effect.Effect<void, EventSourceSetupError, Scope.Scope> {
  return Effect.gen(function* () {
    const session = yield* Effect.tryPromise({
      try: () => ctx.jam.session,
      catch: (cause) => new EventSourceSetupError({ cause }),
    });
    const url = `${session.eventSourceUrl}?types=Email&closeafter=no&ping=60`;
    const source = new JmapEventSource(
      url,
      ctx.jam.authHeader,
      onEmailStateChange,
      logger,
    );

    // acquireRelease registers the finalizer before acquisition can return to
    // its caller. The retaining transport owns the Scope, not an unscoped
    // cleanup value that can be lost to startup interruption.
    yield* Effect.acquireRelease(source.connectEffect, () => source.closeEffect);
  });
}
