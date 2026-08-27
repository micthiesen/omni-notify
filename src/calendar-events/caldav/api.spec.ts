import { Logger } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createCalendarEvent,
  deleteCalendarEvent,
  updateCalendarEvent,
} from "./api.js";

const session = {
  calendarUrl: "https://p42-caldav.icloud.com/123/calendars/home/",
  authHeader: "Basic secret",
};
const event = {
  action: "create" as const,
  title: "Dentist",
  startDate: "2026-09-01",
  startTime: "09:00",
  allDay: false,
};
const logger = new Logger("CalDavApiSpec");

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CalDAV writes", () => {
  it("reconciles a repeated deterministic create after an ambiguous response", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 412 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      createCalendarEvent(session, event, logger, "stable@omni-notify"),
    ).resolves.toEqual({
      status: "already_exists",
      eventUid: "stable@omni-notify",
    });
    expect(fetchMock).toHaveBeenCalledWith(
      `${session.calendarUrl}stable@omni-notify.ics`,
      expect.objectContaining({
        method: "PUT",
        signal: expect.any(AbortSignal),
        headers: expect.objectContaining({ "If-None-Match": "*" }),
      }),
    );
  });

  it("bounds update and delete requests", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(null, { status: 404 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      updateCalendarEvent(session, event, "stable@omni-notify", logger),
    ).resolves.toMatchObject({ status: "success" });
    await expect(
      deleteCalendarEvent(session, "stable@omni-notify", logger),
    ).resolves.toEqual({ status: "not_found" });
    expect(fetchMock.mock.calls[0]?.[1]?.signal).toBeInstanceOf(AbortSignal);
    expect(fetchMock.mock.calls[1]?.[1]?.signal).toBeInstanceOf(AbortSignal);
  });
});
