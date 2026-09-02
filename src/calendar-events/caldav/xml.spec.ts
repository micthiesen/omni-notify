import { describe, expect, it } from "vitest";
import {
  extractCalendarCollections,
  extractPropertyHref,
  pickCalendarCollection,
} from "./xml.js";

const icloudPrincipalXml = `<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/</href>
    <propstat>
      <prop>
        <current-user-principal>
          <href>/123456789/principal/</href>
        </current-user-principal>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>`;

const prefixedHomeSetXml = `<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/123456789/principal/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>https://p42-caldav.icloud.com/123456789/calendars/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>`;

const collectionsXml = `<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/">
  <d:response>
    <d:href>/123456789/calendars/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/123456789/calendars/home/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Home</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VEVENT"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/123456789/calendars/personal-cal/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Personal</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VEVENT"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/123456789/calendars/tasks/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Reminders</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VTODO"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>`;

describe("extractPropertyHref", () => {
  it("finds current-user-principal in unprefixed iCloud XML", () => {
    expect(extractPropertyHref(icloudPrincipalXml, "current-user-principal")).toBe(
      "/123456789/principal/",
    );
  });

  it("finds calendar-home-set in prefixed XML", () => {
    expect(extractPropertyHref(prefixedHomeSetXml, "calendar-home-set")).toBe(
      "https://p42-caldav.icloud.com/123456789/calendars/",
    );
  });

  it("returns undefined when the property is absent", () => {
    expect(
      extractPropertyHref(icloudPrincipalXml, "calendar-home-set"),
    ).toBeUndefined();
  });
});

describe("extractCalendarCollections", () => {
  it("extracts only calendar collections with names and components", () => {
    const collections = extractCalendarCollections(collectionsXml);
    expect(collections).toEqual([
      { href: "/123456789/calendars/home/", name: "Home", components: ["VEVENT"] },
      {
        href: "/123456789/calendars/personal-cal/",
        name: "Personal",
        components: ["VEVENT"],
      },
      {
        href: "/123456789/calendars/tasks/",
        name: "Reminders",
        components: ["VTODO"],
      },
    ]);
  });

  it("handles iCloud's unprefixed xmlns-attribute style with single-quoted comps", () => {
    const xml = `<multistatus xmlns="DAV:">
      <response xmlns="DAV:">
        <href>/285128981/calendars/ABCD-1234/</href>
        <propstat><prop>
          <displayname xmlns="DAV:">Personal</displayname>
          <resourcetype xmlns="DAV:"><collection/><calendar xmlns="urn:ietf:params:xml:ns:caldav"/></resourcetype>
          <supported-calendar-component-set xmlns="urn:ietf:params:xml:ns:caldav"><comp name='VEVENT' xmlns='urn:ietf:params:xml:ns:caldav'/></supported-calendar-component-set>
        </prop><status>HTTP/1.1 200 OK</status></propstat>
      </response>
      <response xmlns="DAV:">
        <href>/285128981/calendars/tasks/</href>
        <propstat><prop>
          <displayname xmlns="DAV:">Shopping / Home</displayname>
          <resourcetype xmlns="DAV:"><collection/><calendar xmlns="urn:ietf:params:xml:ns:caldav"/></resourcetype>
          <supported-calendar-component-set xmlns="urn:ietf:params:xml:ns:caldav"><comp name='VTODO' xmlns='urn:ietf:params:xml:ns:caldav'/></supported-calendar-component-set>
        </prop><status>HTTP/1.1 200 OK</status></propstat>
      </response>
    </multistatus>`;
    expect(extractCalendarCollections(xml)).toEqual([
      {
        href: "/285128981/calendars/ABCD-1234/",
        name: "Personal",
        components: ["VEVENT"],
      },
      {
        href: "/285128981/calendars/tasks/",
        name: "Shopping / Home",
        components: ["VTODO"],
      },
    ]);
  });
});

describe("pickCalendarCollection", () => {
  const collections = extractCalendarCollections(collectionsXml);

  it("prefers the configured name case-insensitively", () => {
    expect(pickCalendarCollection(collections, "personal")?.name).toBe("Personal");
  });

  it("falls back to a default-sounding VEVENT calendar in preference order", () => {
    expect(pickCalendarCollection(collections, undefined)?.name).toBe("Personal");
  });

  it("prefers the account-default calendar named iCloud when present", () => {
    const withDefault = [
      ...collections,
      { href: "/x/calendars/work/", name: "iCloud", components: ["VEVENT"] },
    ];
    expect(pickCalendarCollection(withDefault, undefined)?.name).toBe("iCloud");
  });

  it("never picks a VTODO-only collection", () => {
    expect(pickCalendarCollection(collections, "Reminders")?.name).not.toBe(
      "Reminders",
    );
  });

  it("returns undefined when nothing is event-capable", () => {
    const todoOnly = collections.filter((c) => c.name === "Reminders");
    expect(pickCalendarCollection(todoOnly, undefined)).toBeUndefined();
  });
});
