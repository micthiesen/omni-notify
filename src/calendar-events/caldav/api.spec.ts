import { Logger } from "@micthiesen/mitools/logging";
import { Effect, Fiber } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createCalendarEvent,
  createCalendarEventEffect,
  deleteCalendarEvent,
  deleteCalendarEventEffect,
  updateCalendarEvent,
} from "./api.js";
import { CALDAV_ERROR_MAX_BYTES } from "./http.js";

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

function abortableFetchMock(): {
  fetchMock: ReturnType<typeof vi.fn>;
  started: Promise<AbortSignal>;
} {
  const started = Promise.withResolvers<AbortSignal>();
  const fetchMock = vi.fn(
    (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const signal = init?.signal as AbortSignal;
      started.resolve(signal);
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(signal.reason), { once: true });
      });
    },
  );
  return { fetchMock, started: started.promise };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CalDAV writes", () => {
  it("aborts an in-flight PUT when its Effect is interrupted", async () => {
    const { fetchMock, started } = abortableFetchMock();
    vi.stubGlobal("fetch", fetchMock);

    const fiber = Effect.runFork(
      createCalendarEventEffect(session, event, logger, "stable@omni-notify"),
    );
    const signal = await started;
    expect(signal.aborted).toBe(false);

    await Effect.runPromise(Fiber.interrupt(fiber));

    expect(signal.aborted).toBe(true);
    expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("PUT");
  });

  it("aborts an in-flight DELETE when its Effect is interrupted", async () => {
    const { fetchMock, started } = abortableFetchMock();
    vi.stubGlobal("fetch", fetchMock);

    const fiber = Effect.runFork(
      deleteCalendarEventEffect(session, "stable@omni-notify", logger),
    );
    const signal = await started;
    expect(signal.aborted).toBe(false);

    await Effect.runPromise(Fiber.interrupt(fiber));

    expect(signal.aborted).toBe(true);
    expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("DELETE");
  });

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

  it("rejects an oversized write error response instead of buffering it", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response("failure", {
          status: 500,
          headers: { "Content-Length": String(CALDAV_ERROR_MAX_BYTES + 1) },
        }),
      ),
    );

    await expect(
      createCalendarEvent(session, event, logger, "stable@omni-notify"),
    ).rejects.toThrow(`response exceeds the ${CALDAV_ERROR_MAX_BYTES}-byte limit`);
  });
});
