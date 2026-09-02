import type { Logger } from "@micthiesen/mitools/logging";
import type { EmailTriageService } from "../email/triage.js";
import type { EmailHandler, EmailTransport } from "../email/types.js";
import { getCaldavProvider } from "./caldav/index.js";
import { CalendarEventPipeline } from "./pipeline.js";

export function createCalendarHandler(
  transport: EmailTransport,
  parentLogger: Logger,
  triage: EmailTriageService,
): EmailHandler | undefined {
  const logger = parentLogger.extend("CalendarEvents");

  if (!getCaldavProvider()) {
    logger.info("Disabled: no iCloud CalDAV credentials configured");
    return undefined;
  }

  logger.info("Pipeline created");
  return new CalendarEventPipeline(transport, logger, triage);
}
