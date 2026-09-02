import { Logger } from "@micthiesen/mitools/logging";
import { layer } from "@effect/vitest";
import { Effect, Fiber } from "effect";
import { afterEach, expect, vi } from "vitest";
import {
  createCalendarEventEffect,
  deleteCalendarEventEffect,
  updateCalendarEventEffect,
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
const logger = Logger.named("CalDavApiSpec");

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

layer(Logger.layer())("CalDAV writes", (effectIt) => {
  effectIt.effect("aborts an in-flight PUT when its Effect is interrupted", () =>
    Effect.gen(function* () {
      const { fetchMock, started } = abortableFetchMock();
      vi.stubGlobal("fetch", fetchMock);
      const fiber = yield* Effect.forkChild(
        createCalendarEventEffect(session, event, logger, "stable@omni-notify"),
      );
      const signal = yield* Effect.promise(() => started);
      expect(signal.aborted).toBe(false);
      yield* Fiber.interrupt(fiber);
      expect(signal.aborted).toBe(true);
      expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("PUT");
    }),
  );

  effectIt.effect("aborts an in-flight DELETE when its Effect is interrupted", () =>
    Effect.gen(function* () {
      const { fetchMock, started } = abortableFetchMock();
      vi.stubGlobal("fetch", fetchMock);
      const fiber = yield* Effect.forkChild(
        deleteCalendarEventEffect(session, "stable@omni-notify", logger),
      );
      const signal = yield* Effect.promise(() => started);
      expect(signal.aborted).toBe(false);
      yield* Fiber.interrupt(fiber);
      expect(signal.aborted).toBe(true);
      expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("DELETE");
    }),
  );

  effectIt.effect(
    "reconciles a repeated deterministic create after an ambiguous response",
    () =>
      Effect.gen(function* () {
        const fetchMock = vi
          .fn()
          .mockResolvedValue(new Response(null, { status: 412 }));
        vi.stubGlobal("fetch", fetchMock);
        const result = yield* createCalendarEventEffect(
          session,
          event,
          logger,
          "stable@omni-notify",
        );
        expect(result).toEqual({
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
      }),
  );

  effectIt.effect("bounds update and delete requests", () =>
    Effect.gen(function* () {
      const fetchMock = vi
        .fn()
        .mockResolvedValueOnce(new Response(null, { status: 204 }))
        .mockResolvedValueOnce(new Response(null, { status: 404 }));
      vi.stubGlobal("fetch", fetchMock);
      const update = yield* updateCalendarEventEffect(
        session,
        event,
        "stable@omni-notify",
        logger,
      );
      const deletion = yield* deleteCalendarEventEffect(
        session,
        "stable@omni-notify",
        logger,
      );
      expect(update).toMatchObject({ status: "success" });
      expect(deletion).toEqual({ status: "not_found" });
      expect(fetchMock.mock.calls[0]?.[1]?.signal).toBeInstanceOf(AbortSignal);
      expect(fetchMock.mock.calls[1]?.[1]?.signal).toBeInstanceOf(AbortSignal);
    }),
  );

  effectIt.effect(
    "rejects an oversized write error response instead of buffering it",
    () =>
      Effect.gen(function* () {
        vi.stubGlobal(
          "fetch",
          vi.fn().mockResolvedValue(
            new Response("failure", {
              status: 500,
              headers: { "Content-Length": String(CALDAV_ERROR_MAX_BYTES + 1) },
            }),
          ),
        );
        const error = yield* Effect.flip(
          createCalendarEventEffect(session, event, logger, "stable@omni-notify"),
        );
        expect(error._tag).toBe("CaldavError");
        expect(error.message).toContain(
          `response exceeds the ${CALDAV_ERROR_MAX_BYTES}-byte limit`,
        );
      }),
  );
});
