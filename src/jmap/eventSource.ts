import type { Logger } from "@micthiesen/mitools/logging";
import { EventSource } from "eventsource";
import type { JmapContext } from "./client.js";

export type StateChangeHandler = (accountId: string) => void;

const FETCH_TIMEOUT_MS = 30_000;
const MAX_CONNECTION_AGE_MS = 30 * 60_000;

/**
 * Connect to Fastmail's JMAP event source for real-time email notifications.
 *
 * Fastmail closes the SSE connection after each state push (sends `event: close`),
 * so the `eventsource` library's built-in auto-reconnect handles the polling loop.
 * Each cycle: connect → receive state → connection closes → auto-reconnect.
 *
 * Two resilience mechanisms guard against silent connection death:
 * 1. Fetch timeout: prevents hanging reconnect attempts (DNS issues, network problems)
 * 2. Max connection age: forces a reconnect every 30 min so idle connections don't go
 *    stale undetected (Fastmail doesn't send pings despite the ping parameter)
 */
export async function createEventSource(
  ctx: JmapContext,
  onEmailStateChange: StateChangeHandler | StateChangeHandler[],
  logger: Logger,
): Promise<() => void> {
  const session = await ctx.jam.session;
  const url = `${session.eventSourceUrl}?types=Email&closeafter=no&ping=60`;
  const bearerToken = ctx.jam.authHeader;
  const handlers = Array.isArray(onEmailStateChange)
    ? onEmailStateChange
    : [onEmailStateChange];

  let es: EventSource | null = null;
  let maxAgeTimer: ReturnType<typeof setTimeout> | null = null;
  let closed = false;
  let connected = false;

  function resetMaxAgeTimer() {
    if (maxAgeTimer) clearTimeout(maxAgeTimer);
    if (closed) return;
    maxAgeTimer = setTimeout(() => {
      if (closed) return;
      logger.info("EventSource max connection age reached, forcing reconnect");
      // Close and recreate — the eventsource library won't reconnect after close()
      es?.close();
      createConnection();
    }, MAX_CONNECTION_AGE_MS);
  }

  function createConnection() {
    es = new EventSource(url, {
      fetch: (input, init) => {
        const headers = (init?.headers ?? {}) as Record<string, string>;
        headers.Authorization = bearerToken;
        return globalThis.fetch(input, {
          ...init,
          headers,
          signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
        });
      },
    });

    resetMaxAgeTimer();

    es.addEventListener("open", () => {
      if (!connected) {
        logger.info("EventSource connected");
        connected = true;
      }
      resetMaxAgeTimer();
    });

    es.addEventListener("state", (event) => {
      resetMaxAgeTimer();
      try {
        const data = JSON.parse(event.data) as {
          changed: Record<string, Record<string, string>>;
        };

        for (const [accountId, changes] of Object.entries(data.changed)) {
          if ("Email" in changes) {
            logger.debug(`Email state change for account ${accountId}`);
            for (const handler of handlers) {
              try {
                handler(accountId);
              } catch (error) {
                logger.error("Handler error", (error as Error).message);
              }
            }
          }
        }
      } catch (error) {
        logger.error("Failed to parse state change event", (error as Error).message);
      }
    });

    es.addEventListener("error", (event) => {
      const code = (event as { code?: number }).code;
      const message = (event as { message?: string }).message;

      if (code === 401 || code === 403) {
        logger.error(
          "EventSource auth error, closing connection",
          `Code ${code}: ${message}`,
        );
        close();
        return;
      }

      // Connection-close errors are expected — Fastmail closes after each state push
      // and the eventsource library auto-reconnects. Only log real errors.
      if (message) {
        logger.warn(`EventSource error: ${message}`);
      }
    });
  }

  function close() {
    closed = true;
    if (maxAgeTimer) clearTimeout(maxAgeTimer);
    logger.info("Closing EventSource connection");
    es?.close();
  }

  createConnection();

  return close;
}
