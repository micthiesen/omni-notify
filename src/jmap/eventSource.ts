import type { Logger } from "@micthiesen/mitools/logging";
import { EventSource } from "eventsource";
import type { JmapContext } from "./client.js";

export type StateChangeHandler = (accountId: string) => void;

/**
 * Connect to Fastmail's JMAP event source for real-time email notifications.
 *
 * Fastmail closes the SSE connection after each state push (sends `event: close`),
 * so the `eventsource` library's built-in auto-reconnect handles the polling loop.
 * Each cycle: connect → receive state → connection closes → auto-reconnect.
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

  let connected = false;

  const es = new EventSource(url, {
    fetch: (input, init) => {
      const headers = (init?.headers ?? {}) as Record<string, string>;
      headers.Authorization = bearerToken;
      return globalThis.fetch(input, { ...init, headers });
    },
  });

  es.addEventListener("open", () => {
    if (!connected) {
      logger.info("EventSource connected");
      connected = true;
    }
  });

  es.addEventListener("state", (event) => {
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
      es.close();
      return;
    }

    // Connection-close errors are expected — Fastmail closes after each state push
    // and the eventsource library auto-reconnects. Only log real errors.
    if (message) {
      logger.warn(`EventSource error: ${message}`);
    }
  });

  return () => {
    logger.info("Closing EventSource connection");
    es.close();
  };
}
