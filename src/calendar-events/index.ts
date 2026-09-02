import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import type { EmailTriageService } from "../email/triage.js";
import type { EmailTransport } from "../email/types.js";
import type { TaskServices } from "../task-runs/registry.js";
import { getCaldavProvider } from "./caldav/index.js";
import { CalendarEventPipeline } from "./pipeline.js";

export const createCalendarHandler = Effect.fn("CalendarEvents.createHandler")(
  function* (
    transport: EmailTransport<unknown, TaskServices>,
    parentLogger: NamedLogger,
    triage: EmailTriageService,
  ) {
    const logger = parentLogger.extend("CalendarEvents");

    if (!getCaldavProvider()) {
      yield* logger.info("Disabled: no iCloud CalDAV credentials configured");
      return undefined;
    }

    yield* logger.info("Pipeline created");
    return new CalendarEventPipeline(transport, logger, triage);
  },
);
