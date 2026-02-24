import type { Logger } from "@micthiesen/mitools/logging";
import { EventSource } from "eventsource";
import type { JmapContext } from "./client.js";

export type StateChangeHandler = (accountId: string) => void;

export async function createEventSource(
  ctx: JmapContext,
  onEmailStateChange: StateChangeHandler,
  logger: Logger,
): Promise<() => void> {
  const session = await ctx.jam.session;

  // Build EventSource URL from session template
  const url = session.eventSourceUrl
    .replace("{types}", "Email")
    .replace("{closeafter}", "no")
    .replace("{ping}", "60");

  // Inject Authorization header via custom fetch
  const bearerToken = ctx.jam.authHeader;
  const es = new EventSource(url, {
    fetch: (input, init) => {
      const headers = (init?.headers ?? {}) as Record<string, string>;
      headers.Authorization = bearerToken;
      return globalThis.fetch(input, { ...init, headers });
    },
  });

  let consecutiveErrors = 0;

  es.addEventListener("open", () => {
    consecutiveErrors = 0;
    logger.info("EventSource connected");
  });

  es.addEventListener("state", (event) => {
    try {
      const data = JSON.parse(event.data) as {
        changed: Record<string, Record<string, string>>;
      };

      for (const [accountId, changes] of Object.entries(data.changed)) {
        if ("Email" in changes) {
          logger.debug(`Email state change for account ${accountId}`);
          onEmailStateChange(accountId);
        }
      }
      consecutiveErrors = 0;
    } catch (error) {
      logger.error(`Failed to parse state change event: ${(error as Error).message}`);
    }
  });

  es.addEventListener("error", (event) => {
    consecutiveErrors++;
    const code = (event as { code?: number }).code;
    const message = (event as { message?: string }).message ?? "Unknown error";

    if (code === 401 || code === 403) {
      logger.error(`EventSource auth error (${code}): ${message}. Closing connection.`);
      es.close();
      return;
    }

    if (consecutiveErrors >= 10) {
      logger.error(`EventSource error (${consecutiveErrors} consecutive): ${message}`);
    } else if (consecutiveErrors >= 3) {
      logger.warn(`EventSource error (${consecutiveErrors} consecutive): ${message}`);
    } else {
      logger.debug(`EventSource error: ${message}`);
    }
  });

  return () => {
    logger.info("Closing EventSource connection");
    es.close();
  };
}
