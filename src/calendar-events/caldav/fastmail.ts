import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import config from "../../utils/config.js";
import { CaldavError } from "../effect.js";
import type { CaldavSession } from "./api.js";
import { assertTrustedCaldavUrl, basicAuth, propfindEffect } from "./http.js";
import { extractCalendarCollections, pickCalendarCollection } from "./xml.js";

const CALDAV_HOST = "https://caldav.fastmail.com";
const CALDAV_BASE = `${CALDAV_HOST}/dav/calendars`;

/**
 * Fastmail CalDAV: calendar URLs are username-based, so discovery is a single
 * Depth:1 PROPFIND on the user's calendar home (or none at all when
 * FASTMAIL_CALENDAR_ID pins the collection).
 */
export function discoverFastmailCalendarEffect(
  logger: Logger,
): Effect.Effect<CaldavSession, CaldavError> {
  return Effect.gen(function* () {
    const authHeader = basicAuth(
      config.FASTMAIL_USERNAME ?? "",
      config.FASTMAIL_APP_PASSWORD ?? "",
    );

    if (config.FASTMAIL_CALENDAR_ID) {
      const url = `${CALDAV_BASE}/user/${config.FASTMAIL_USERNAME}/${config.FASTMAIL_CALENDAR_ID}/`;
      logger.info(`Using configured calendar: ${url}`);
      return { calendarUrl: url, authHeader };
    }

    const homeUrl = `${CALDAV_BASE}/user/${config.FASTMAIL_USERNAME}/`;
    const { xml, url } = yield* propfindEffect(
      homeUrl,
      authHeader,
      "1",
      `<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>`,
    );

    const collections = extractCalendarCollections(xml);
    const selected = pickCalendarCollection(collections, undefined);
    if (!selected) {
      return yield* new CaldavError({
        operation: "discover Fastmail calendar",
        cause: "No calendars found via Fastmail CalDAV PROPFIND",
        transient: false,
      });
    }

    const calendarUrl = assertTrustedCaldavUrl(
      new URL(selected.href, url).toString(),
      "fastmail",
    );
    logger.info(`Using calendar: ${selected.name} (${calendarUrl})`);
    return { calendarUrl, authHeader };
  });
}
