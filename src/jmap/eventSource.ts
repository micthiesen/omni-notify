import type { Logger } from "@micthiesen/mitools/logging";
import { EventSource } from "eventsource";
import type { JmapContext } from "./client.js";

export type StateChangeHandler = (accountId: string) => void;

const MIN_BACKOFF_MS = 1_000;
const MAX_BACKOFF_MS = 5 * 60_000;
const HEARTBEAT_TIMEOUT_MS = 5 * 60_000;

export async function createEventSource(
  ctx: JmapContext,
  onEmailStateChange: StateChangeHandler | StateChangeHandler[],
  logger: Logger,
): Promise<() => void> {
  const session = await ctx.jam.session;
  const url = session.eventSourceUrl
    .replace("{types}", "Email")
    .replace("{closeafter}", "no")
    .replace("{ping}", "60");
  const bearerToken = ctx.jam.authHeader;
  const handlers = Array.isArray(onEmailStateChange)
    ? onEmailStateChange
    : [onEmailStateChange];

  let stopped = false;
  let currentEs: EventSource | null = null;
  let heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let backoffMs = MIN_BACKOFF_MS;
  let consecutiveErrors = 0;

  function resetHeartbeat(): void {
    if (heartbeatTimer) clearTimeout(heartbeatTimer);
    if (stopped) return;
    heartbeatTimer = setTimeout(() => {
      logger.warn(
        `No EventSource activity for ${HEARTBEAT_TIMEOUT_MS / 1000}s, reconnecting`,
      );
      reconnect();
    }, HEARTBEAT_TIMEOUT_MS);
  }

  function connect(): void {
    if (stopped) return;

    const es = new EventSource(url, {
      fetch: (input, init) => {
        const headers = (init?.headers ?? {}) as Record<string, string>;
        headers.Authorization = bearerToken;
        return globalThis.fetch(input, { ...init, headers });
      },
    });
    currentEs = es;

    es.addEventListener("open", () => {
      consecutiveErrors = 0;
      backoffMs = MIN_BACKOFF_MS;
      logger.info("EventSource connected");
      resetHeartbeat();
    });

    es.addEventListener("state", (event) => {
      resetHeartbeat();
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
        consecutiveErrors = 0;
      } catch (error) {
        logger.error("Failed to parse state change event", (error as Error).message);
      }
    });

    // Pings also reset the heartbeat
    es.addEventListener("ping", () => {
      resetHeartbeat();
    });

    es.addEventListener("error", (event) => {
      consecutiveErrors++;
      const code = (event as { code?: number }).code;
      const message = (event as { message?: string }).message ?? "Unknown error";

      if (code === 401 || code === 403) {
        logger.error(
          "EventSource auth error (not reconnecting)",
          `Code ${code}: ${message}`,
        );
        cleanup();
        return;
      }

      if (consecutiveErrors >= 10) {
        logger.error(
          `EventSource error (${consecutiveErrors} consecutive), reconnecting`,
          message,
        );
        reconnect();
      } else if (consecutiveErrors >= 3) {
        logger.warn(`EventSource error (${consecutiveErrors} consecutive): ${message}`);
      } else {
        logger.debug(`EventSource error: ${message}`);
      }
    });
  }

  function reconnect(): void {
    if (stopped) return;

    if (currentEs) {
      currentEs.close();
      currentEs = null;
    }
    if (heartbeatTimer) clearTimeout(heartbeatTimer);

    logger.info(`Reconnecting in ${(backoffMs / 1000).toFixed(0)}s`);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, backoffMs);

    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
  }

  function cleanup(): void {
    stopped = true;
    if (reconnectTimer) clearTimeout(reconnectTimer);
    if (heartbeatTimer) clearTimeout(heartbeatTimer);
    if (currentEs) {
      logger.info("Closing EventSource connection");
      currentEs.close();
      currentEs = null;
    }
  }

  connect();
  return cleanup;
}
