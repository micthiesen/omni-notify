/**
 * Minimal CalDAV multistatus XML parsing. Regex-based on purpose: the two
 * servers we talk to (Fastmail, iCloud) emit simple, flat PROPFIND responses,
 * and namespace prefixes vary per server (d:, D:, none, A:, ...) so matching
 * is prefix-agnostic. Pure functions, tested in xml.spec.ts.
 */

const PREFIX = "(?:[\\w-]+:)?";

/**
 * Extract the href inside a named property element, e.g.
 * <current-user-principal><href>/123/principal/</href></current-user-principal>.
 */
export function extractPropertyHref(xml: string, property: string): string | undefined {
  const block = new RegExp(
    `<${PREFIX}${property}[^>]*>([\\s\\S]*?)</${PREFIX}${property}>`,
    "i",
  ).exec(xml);
  if (!block) return undefined;
  const href = new RegExp(`<${PREFIX}href[^>]*>([^<]+)</${PREFIX}href>`, "i").exec(
    block[1],
  );
  return href?.[1].trim() || undefined;
}

export interface CalendarCollection {
  href: string;
  name: string;
  /** VEVENT/VTODO/... when the server reported a component set; else undefined. */
  components?: string[];
}

/**
 * Extract calendar collections from a Depth:1 PROPFIND multistatus response.
 * A response counts as a calendar when its resourcetype contains a bare
 * <calendar/> element (calendar-proxy-* and friends don't match).
 */
export function extractCalendarCollections(xml: string): CalendarCollection[] {
  const results: CalendarCollection[] = [];

  const responseBlocks = xml
    .split(new RegExp(`<${PREFIX}response[>\\s]`, "i"))
    .slice(1);

  for (const block of responseBlocks) {
    const resourcetype = new RegExp(
      `<${PREFIX}resourcetype[^>]*>([\\s\\S]*?)</${PREFIX}resourcetype>`,
      "i",
    ).exec(block);
    // Match <calendar/>, <c:calendar/>, and iCloud's attribute-carrying
    // <calendar xmlns="urn:ietf:params:xml:ns:caldav"/> — but never
    // <calendar-proxy-*> (the lookahead requires the name to end there).
    const isCalendar =
      resourcetype !== null &&
      new RegExp(`<${PREFIX}calendar(?=[\\s/>])[^>]*>`, "i").test(resourcetype[1]);
    if (!isCalendar) continue;

    const hrefMatch = new RegExp(
      `<${PREFIX}href[^>]*>([^<]+)</${PREFIX}href>`,
      "i",
    ).exec(block);
    if (!hrefMatch) continue;

    const nameMatch = new RegExp(
      `<${PREFIX}displayname[^>]*>(?:<!\\[CDATA\\[)?(.*?)(?:\\]\\]>)?</${PREFIX}displayname>`,
      "i",
    ).exec(block);
    const name = nameMatch?.[1] ?? "Unnamed";

    const componentsBlock = new RegExp(
      `<${PREFIX}supported-calendar-component-set[^>]*>([\\s\\S]*?)</${PREFIX}supported-calendar-component-set>`,
      "i",
    ).exec(block);
    let components: string[] | undefined;
    if (componentsBlock) {
      // iCloud single-quotes the attribute (<comp name='VTODO'/>).
      components = [...componentsBlock[1].matchAll(/name=["']([A-Z]+)["']/gi)].map(
        (m) => m[1].toUpperCase(),
      );
    }

    results.push({ href: hrefMatch[1].trim(), name, components });
  }

  return results;
}

/**
 * Pick the collection to write events to: configured name first
 * (case-insensitive), then common defaults, then the first VEVENT-capable
 * collection. Collections reporting a component set without VEVENT (task
 * lists, reminders) are never picked.
 */
export function pickCalendarCollection(
  collections: CalendarCollection[],
  preferredName: string | undefined,
): CalendarCollection | undefined {
  const eventCapable = collections.filter(
    (c) => c.components === undefined || c.components.includes("VEVENT"),
  );

  if (preferredName) {
    const preferred = eventCapable.find(
      (c) => c.name.toLowerCase() === preferredName.toLowerCase(),
    );
    if (preferred) return preferred;
  }

  // In preference order; "iCloud" is the account-default calendar's name.
  for (const fallback of ["icloud", "default", "personal", "home"]) {
    const match = eventCapable.find((c) => c.name.toLowerCase() === fallback);
    if (match) return match;
  }
  return eventCapable[0];
}
