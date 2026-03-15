import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "../jmap/client.js";
import type { StateChangeHandler } from "../jmap/eventSource.js";
import config from "../utils/config.js";
import { CalendarEventPipeline } from "./pipeline.js";

export function createCalendarPipeline(
  ctx: JmapContext,
  parentLogger: Logger,
): StateChangeHandler | undefined {
  const logger = parentLogger.extend("CalendarEvents");

  if (!config.FASTMAIL_USERNAME) {
    logger.info("Disabled: missing FASTMAIL_USERNAME (required for CalDAV)");
    return undefined;
  }

  const pipeline = new CalendarEventPipeline(ctx, logger);

  logger.info("Pipeline created");
  return () => {
    pipeline.onEmailStateChange().catch((error) => {
      logger.error("Pipeline error", (error as Error).message);
    });
  };
}
