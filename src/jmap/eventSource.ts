import type { Logger } from "@micthiesen/mitools/logging";
import { EventSource } from "eventsource";
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
  private inactivityTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private backoffMs = 0;
  private connected = false;
  private closed = false;

  constructor(
    private readonly url: string,
    private readonly bearerToken: string,
    private readonly onEmailStateChange: () => void,
    private readonly logger: Logger,
  ) {}

  connect(): void {
    this.es = new EventSource(this.url, {
      fetch: (input, init) => {
        const headers = (init?.headers ?? {}) as Record<string, string>;
        headers.Authorization = this.bearerToken;
        return globalThis.fetch(input, { ...init, headers });
      },
    });

    this.resetInactivityTimer();

    this.es.addEventListener("open", () => {
      this.backoffMs = INITIAL_BACKOFF_MS;
      if (!this.connected) {
        this.logger.info("EventSource connected");
        this.connected = true;
      }
      this.resetInactivityTimer();
    });

    // RFC 8620 §7.3: `event: state` with JSON payload
    // { changed: { [accountId]: { [dataType]: newState } } }
    this.es.addEventListener("state", (event) => {
      this.resetInactivityTimer();
      this.handleStateEvent(event);
    });

    // RFC 8620 §7.3: `event: ping` with { interval: <ms> } payload.
    // Serves as a keepalive heartbeat to prove the connection is alive.
    this.es.addEventListener("ping", () => {
      this.resetInactivityTimer();
    });

    // Fastmail closes the SSE connection after each state push. The `eventsource`
    // library fires an error event for these disconnects and auto-reconnects.
    // We reset the inactivity timer here since even error events prove liveness.
    this.es.addEventListener("error", (event) => {
      this.resetInactivityTimer();
      this.handleErrorEvent(event);
    });
  }

  close(): void {
    this.closed = true;
    if (this.inactivityTimer) clearTimeout(this.inactivityTimer);
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.logger.info("Closing EventSource connection");
    this.es?.close();
  }

  private handleStateEvent(event: MessageEvent): void {
    try {
      const data = JSON.parse(event.data) as {
        changed: Record<string, Record<string, string>>;
      };

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
      this.close();
      return;
    }

    // Connection-close errors are expected (see Fastmail behavior note on connect()).
    // Only log when there's an actual error message.
    if (message) {
      this.logger.warn(`EventSource error: ${message}`);
    }
  }

  private resetInactivityTimer(): void {
    if (this.inactivityTimer) clearTimeout(this.inactivityTimer);
    if (this.closed) return;
    this.inactivityTimer = setTimeout(() => {
      if (this.closed) return;
      this.logger.warn("EventSource inactivity timeout, forcing reconnect");
      this.reconnect();
    }, INACTIVITY_TIMEOUT_MS);
  }

  private reconnect(): void {
    this.es?.close();
    if (this.closed) return;

    // First reconnect uses random jitter (0-3s) to avoid thundering herd.
    // Subsequent attempts double the delay, capped at MAX_BACKOFF_MS.
    if (this.backoffMs === 0) {
      this.backoffMs = Math.round(Math.random() * 3_000);
    } else {
      this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
    }

    this.logger.debug(`EventSource reconnecting in ${this.backoffMs}ms`);
    this.reconnectTimer = setTimeout(() => {
      if (this.closed) return;
      this.connect();
    }, this.backoffMs);
  }
}

/**
 * Create a resilient JMAP EventSource connection. Returns a cleanup function
 * that closes the connection and cancels all timers.
 */
export async function createEventSource(
  ctx: JmapContext,
  onEmailStateChange: () => void,
  logger: Logger,
): Promise<() => void> {
  const session = await ctx.jam.session;
  const url = `${session.eventSourceUrl}?types=Email&closeafter=no&ping=60`;

  const source = new JmapEventSource(
    url,
    ctx.jam.authHeader,
    onEmailStateChange,
    logger,
  );
  source.connect();

  return () => source.close();
}
