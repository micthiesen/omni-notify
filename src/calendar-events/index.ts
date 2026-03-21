import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "../jmap/client.js";
import type { EmailHandler } from "../jmap/dispatcher.js";
import config from "../utils/config.js";
import { CalendarEventPipeline } from "./pipeline.js";

export function createCalendarHandler(
  ctx: JmapContext,
  parentLogger: Logger,
): EmailHandler | undefined {
  const logger = parentLogger.extend("CalendarEvents");

  if (!config.FASTMAIL_USERNAME) {
    logger.info("Disabled: missing FASTMAIL_USERNAME (required for CalDAV)");
    return undefined;
  }

  logger.info("Pipeline created");
  return new CalendarEventPipeline(ctx, logger);
}
