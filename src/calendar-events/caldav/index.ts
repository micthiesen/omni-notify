import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import config from "../../utils/config.js";
import { CaldavError } from "../effect.js";
import type { CaldavSession } from "./api.js";
import { discoverFastmailCalendarEffect } from "./fastmail.js";
import { discoverIcloudCalendarEffect } from "./icloud.js";

export {
  type CaldavSession,
  createCalendarEventEffect,
  deleteCalendarEventEffect,
  updateCalendarEventEffect,
} from "./api.js";

export type CaldavProviderName = "fastmail" | "icloud";

/**
 * Which CalDAV backend calendar events are written to. CALDAV_PROVIDER
 * overrides (calendar data migrated to iCloud before the mail cutover);
 * otherwise it follows the resolved email transport. Returns undefined when
 * the chosen provider's credentials aren't configured.
 */
export function getCaldavProvider(): CaldavProviderName | undefined {
  const chosen = config.CALDAV_PROVIDER ?? config.EMAIL_TRANSPORT;
  if (chosen === "fastmail") {
    return config.FASTMAIL_USERNAME && config.FASTMAIL_APP_PASSWORD
      ? "fastmail"
      : undefined;
  }
  if (chosen === "icloud") {
    return config.ICLOUD_USERNAME && config.ICLOUD_APP_PASSWORD ? "icloud" : undefined;
  }
  return undefined;
}

export function discoverCaldavSessionEffect(
  logger: Logger,
): Effect.Effect<CaldavSession, CaldavError> {
  const provider = getCaldavProvider();
  switch (provider) {
    case "fastmail":
      return discoverFastmailCalendarEffect(logger);
    case "icloud":
      return discoverIcloudCalendarEffect(logger);
    default:
      return Effect.fail(
        new CaldavError({
          operation: "discover calendar",
          cause: "No CalDAV provider configured",
          transient: false,
        }),
      );
  }
}
