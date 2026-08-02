import type { Logger } from "@micthiesen/mitools/logging";
import config from "../../utils/config.js";
import type { CaldavSession } from "./api.js";
import { discoverFastmailCalendar } from "./fastmail.js";
import { discoverIcloudCalendar } from "./icloud.js";

export {
  type CaldavSession,
  createCalendarEvent,
  deleteCalendarEvent,
  updateCalendarEvent,
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

/** Resolve the active provider's calendar collection + auth. */
export async function discoverCaldavSession(logger: Logger): Promise<CaldavSession> {
  const provider = getCaldavProvider();
  switch (provider) {
    case "fastmail":
      return discoverFastmailCalendar(logger);
    case "icloud":
      return discoverIcloudCalendar(logger);
    default:
      throw new Error("No CalDAV provider configured");
  }
}
