import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import type { EmailTriageService } from "../email/triage.js";
import config from "../utils/config.js";
import { DeliveryPipeline } from "./pipeline.js";

export const createParcelHandler = Effect.fn("ParcelTracker.createHandler")(function* (
  parentLogger: NamedLogger,
  triage: EmailTriageService,
) {
  const logger = parentLogger.extend("ParcelTracker");

  if (!config.PARCEL_API_KEY) {
    yield* logger.info("Disabled: missing PARCEL_API_KEY");
    return undefined;
  }

  yield* logger.info("Pipeline created");
  return new DeliveryPipeline(config.PARCEL_API_KEY, logger, triage);
});
