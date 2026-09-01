import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import config from "../../utils/config.js";
import { CaldavError } from "../effect.js";
import type { CaldavSession } from "./api.js";
import { assertTrustedCaldavUrl, basicAuth, propfindEffect } from "./http.js";
import {
  extractCalendarCollections,
  extractPropertyHref,
  pickCalendarCollection,
} from "./xml.js";

const ICLOUD_CALDAV_ROOT = "https://caldav.icloud.com/";

/**
 * iCloud CalDAV discovery. Unlike Fastmail there are no username-based URLs:
 * the well-known root redirects to a per-account shard
 * (https://pXX-caldav.icloud.com/<dsid>/...), so we must follow the RFC 6764
 * chain — current-user-principal → calendar-home-set → calendar collections —
 * and never hardcode the shard. ICLOUD_CALENDAR_URL short-circuits everything
 * (paste a previously discovered collection URL); ICLOUD_CALENDAR_NAME picks
 * the collection by display name (e.g. "Personal").
 *
 * Auth is the same app-specific password as IMAP, against the iCloud primary
 * username (micthiesen@icloud.com style), not the custom-domain address.
 */
export function discoverIcloudCalendarEffect(
  logger: Logger,
): Effect.Effect<CaldavSession, CaldavError> {
  return Effect.gen(function* () {
    const username = config.ICLOUD_USERNAME ?? "";
    const password = config.ICLOUD_APP_PASSWORD ?? "";
    const authHeader = basicAuth(username, password);

    if (config.ICLOUD_CALENDAR_URL) {
      const configuredUrl = config.ICLOUD_CALENDAR_URL.endsWith("/")
        ? config.ICLOUD_CALENDAR_URL
        : `${config.ICLOUD_CALENDAR_URL}/`;
      const url = assertTrustedCaldavUrl(configuredUrl, "icloud");
      logger.info(`Using configured iCloud calendar: ${url}`);
      return { calendarUrl: url, authHeader };
    }

    // 1. Principal discovery at the well-known root (redirects to the shard).
    const principalResponse = yield* propfindEffect(
      ICLOUD_CALDAV_ROOT,
      authHeader,
      "0",
      `<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop><d:current-user-principal/></d:prop>
</d:propfind>`,
    );
    const principalHref = extractPropertyHref(
      principalResponse.xml,
      "current-user-principal",
    );
    if (!principalHref) {
      return yield* new CaldavError({
        operation: "discover iCloud principal",
        cause: "no current-user-principal in PROPFIND response",
        transient: false,
      });
    }
    const principalUrl = assertTrustedCaldavUrl(
      new URL(principalHref, principalResponse.url).toString(),
      "icloud",
    );
    logger.debug(`iCloud CalDAV principal: ${principalUrl}`);

    // 2. Calendar home on the principal.
    const homeResponse = yield* propfindEffect(
      principalUrl,
      authHeader,
      "0",
      `<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><c:calendar-home-set/></d:prop>
</d:propfind>`,
    );
    const homeHref = extractPropertyHref(homeResponse.xml, "calendar-home-set");
    if (!homeHref) {
      return yield* new CaldavError({
        operation: "discover iCloud calendar home",
        cause: "no calendar-home-set on principal",
        transient: false,
      });
    }
    const homeUrl = assertTrustedCaldavUrl(
      new URL(homeHref, homeResponse.url).toString(),
      "icloud",
    );
    logger.debug(`iCloud CalDAV calendar home: ${homeUrl}`);

    // 3. List collections; filter to VEVENT-capable calendars (iCloud exposes
    //    reminder/task lists in the same home).
    const listResponse = yield* propfindEffect(
      homeUrl,
      authHeader,
      "1",
      `<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <c:supported-calendar-component-set/>
  </d:prop>
</d:propfind>`,
    );
    const collections = extractCalendarCollections(listResponse.xml);
    const selected = pickCalendarCollection(collections, config.ICLOUD_CALENDAR_NAME);
    if (!selected) {
      const names = collections.map((c) => c.name).join(", ") || "none";
      return yield* new CaldavError({
        operation: "discover iCloud calendar collection",
        cause: `no usable calendar collection found (saw: ${names})`,
        transient: false,
      });
    }

    const calendarUrl = assertTrustedCaldavUrl(
      new URL(selected.href, listResponse.url).toString(),
      "icloud",
    );
    logger.info(`Using iCloud calendar: ${selected.name} (${calendarUrl})`);
    return { calendarUrl, authHeader };
  });
}
