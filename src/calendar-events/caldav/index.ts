import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import config from "../../utils/config.js";
import { CaldavError } from "../effect.js";
import type { CaldavSession } from "./api.js";
import { discoverIcloudCalendarEffect } from "./icloud.js";

export {
  type CaldavSession,
  createCalendarEventEffect,
  deleteCalendarEventEffect,
  updateCalendarEventEffect,
} from "./api.js";

export type CaldavProviderName = "icloud";

/**
 * Returns the iCloud CalDAV backend when its credentials are configured.
 */
export function getCaldavProvider(): CaldavProviderName | undefined {
  return config.ICLOUD_USERNAME && config.ICLOUD_APP_PASSWORD ? "icloud" : undefined;
}

export function discoverCaldavSessionEffect(
  logger: Logger,
): Effect.Effect<CaldavSession, CaldavError> {
  const provider = getCaldavProvider();
  return provider
    ? discoverIcloudCalendarEffect(logger)
    : Effect.fail(
        new CaldavError({
          operation: "discover calendar",
          cause: "No CalDAV provider configured",
          transient: false,
        }),
      );
}
