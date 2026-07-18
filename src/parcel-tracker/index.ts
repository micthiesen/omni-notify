import type { Logger } from "@micthiesen/mitools/logging";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { EmailTriageService } from "../jmap/triage.js";
import config from "../utils/config.js";
import { DeliveryPipeline } from "./pipeline.js";

export function createParcelHandler(
  parentLogger: Logger,
  triage: EmailTriageService,
): EmailHandler | undefined {
  const logger = parentLogger.extend("ParcelTracker");

  if (!config.PARCEL_API_KEY) {
    logger.info("Disabled: missing PARCEL_API_KEY");
    return undefined;
  }

  logger.info("Pipeline created");
  return new DeliveryPipeline(config.PARCEL_API_KEY, logger, triage);
}
