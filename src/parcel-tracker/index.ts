import type { Logger } from "@micthiesen/mitools/logging";
import config from "../utils/config.js";
import { createJmapClient } from "./jmap/client.js";
import { createEventSource } from "./jmap/eventSource.js";
import { DeliveryPipeline } from "./pipeline.js";

export async function startParcelTracker(
  parentLogger: Logger,
): Promise<(() => void) | undefined> {
  const logger = parentLogger.extend("ParcelTracker");

  if (!config.FASTMAIL_API_TOKEN || !config.PARCEL_API_KEY) {
    logger.info("Disabled: missing FASTMAIL_API_TOKEN or PARCEL_API_KEY");
    return undefined;
  }

  const fastmailToken = config.FASTMAIL_API_TOKEN;
  const parcelApiKey = config.PARCEL_API_KEY;

  const ctx = await createJmapClient(fastmailToken, logger);
  const pipeline = new DeliveryPipeline(ctx, parcelApiKey, logger);

  const closeEventSource = await createEventSource(
    ctx,
    () => {
      pipeline.onEmailStateChange().catch((error) => {
        logger.error(`Pipeline error: ${(error as Error).message}`);
      });
    },
    logger,
  );

  logger.info("Started");
  return closeEventSource;
}
