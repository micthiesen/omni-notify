import { Logger } from "@micthiesen/mitools/logging";
import { Cause, Deferred, Effect, Exit, Fiber } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CalendarExtractionError, CalendarPersistenceError } from "./effect.js";

const mocks = vi.hoisted(() => ({
  enqueue: vi.fn(),
  record: vi.fn(),
  discover: vi.fn(),
  extract: vi.fn(),
  create: vi.fn(),
  recordCreated: vi.fn(),
  notify: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../email/retry.js", () => ({ enqueueEmailRetry: mocks.enqueue }));
vi.mock("@micthiesen/mitools/pushover", () => ({ notify: mocks.notify }));
vi.mock("../email/activity.js", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  recordEmailActivity: mocks.record,
}));
vi.mock("../email/activityLogs.js", () => ({
  withEmailLogCaptureEffect: (
    _key: string,
    _pipeline: string,
    run: () => Effect.Effect<void>,
  ) => run(),
}));
vi.mock("./filter/keywords.js", () => ({
  filterCalendarCandidateEffect: () =>
    Effect.succeed({ pass: true, reason: "test", admitTier: "rules" }),
}));
vi.mock("./caldav/index.js", () => ({
  discoverCaldavSessionEffect: () =>
    Effect.tryPromise({ try: () => mocks.discover(), catch: (cause) => cause }),
  createCalendarEventEffect: (...args: unknown[]) => mocks.create(...args),
  deleteCalendarEventEffect: vi.fn(),
  updateCalendarEventEffect: vi.fn(),
}));
vi.mock("./extraction/extractEvents.js", () => ({
  extractCalendarEventsEffect: mocks.extract,
}));
vi.mock("./persistence.js", () => ({
  reconcileEventHashesEffect: () => Effect.succeed(0),
  getRecentEventsEffect: () => Effect.succeed([]),
  computeEventHash: () => "stable-event-hash",
  computeCalendarEventUid: () => "omni-stable@omni-notify",
  getAllTrackingNumbers: () => new Set(),
  hasCreatedEventEffect: () => Effect.succeed(false),
  hasEventChanged: () => false,
  markEventCancelledEffect: () => Effect.void,
  recordCreatedEventEffect: (...args: unknown[]) =>
    Effect.try({
      try: () => mocks.recordCreated(...args),
      catch: (cause) =>
        new CalendarPersistenceError({ operation: "record created event", cause }),
    }),
  replaceCreatedEventEffect: (...args: unknown[]) =>
    Effect.try({
      try: () => mocks.recordCreated(...args),
      catch: (cause) =>
        new CalendarPersistenceError({ operation: "replace created event", cause }),
    }),
  resolveEventReference: vi.fn(),
  resolveExplicitEventReference: vi.fn(),
}));

describe("CalendarEventPipeline reliability", () => {
  beforeEach(() => vi.clearAllMocks());

  it("durably queues admitted email when calendar discovery fails", async () => {
    mocks.discover.mockRejectedValueOnce(new Error("iCloud offline"));
    const { CalendarEventPipeline } = await import("./pipeline.js");
    const pipeline = new CalendarEventPipeline(
      {} as never,
      new Logger("CalendarPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );

    await Effect.runPromise(
      pipeline.handleEmailsEffect([
        {
          id: "mail-1",
          subject: "Appointment",
          from: "clinic@example.com",
          textBody: "Tomorrow",
          links: [],
          receivedAt: "2026-09-01T00:00:00Z",
          attachments: [],
        },
      ]),
    );

    expect(mocks.enqueue).toHaveBeenCalledWith({
      pipeline: "CalendarEvents",
      emailId: "mail-1",
      reason: "calendar discovery failed: iCloud offline",
    });
  });

  it("durably queues admitted email after transient extraction failure", async () => {
    mocks.discover.mockResolvedValueOnce({
      calendarUrl: "https://caldav.example/calendar/",
      authHeader: "Basic test",
    });
    mocks.extract.mockReturnValueOnce(
      Effect.fail(
        new CalendarExtractionError({
          cause: new Error("model timeout"),
          transient: true,
        }),
      ),
    );
    const { CalendarEventPipeline } = await import("./pipeline.js");
    const pipeline = new CalendarEventPipeline(
      { downloadAttachment: vi.fn() } as never,
      new Logger("CalendarPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );

    await Effect.runPromise(
      pipeline.handleEmailsEffect([
        {
          id: "mail-3",
          subject: "Appointment",
          from: "clinic@example.com",
          textBody: "Tomorrow",
          links: [],
          receivedAt: "2026-09-01T00:00:00Z",
          attachments: [],
        },
      ]),
    );

    expect(mocks.enqueue).toHaveBeenCalledWith({
      pipeline: "CalendarEvents",
      emailId: "mail-3",
      reason: "Calendar extraction failed: model timeout",
    });
  });

  it("preserves interruption while event extraction is in progress", async () => {
    mocks.discover.mockResolvedValueOnce({
      calendarUrl: "https://caldav.example/calendar/",
      authHeader: "Basic test",
    });
    const started = await Effect.runPromise(Deferred.make<void>());
    mocks.extract.mockReturnValueOnce(
      Deferred.succeed(started, undefined).pipe(Effect.andThen(Effect.never)),
    );
    const { CalendarEventPipeline } = await import("./pipeline.js");
    const pipeline = new CalendarEventPipeline(
      { downloadAttachment: vi.fn() } as never,
      new Logger("CalendarPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );

    const exit = await Effect.runPromise(
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(
          pipeline.handleEmailsEffect([
            {
              id: "mail-interrupted",
              subject: "Appointment",
              from: "clinic@example.com",
              textBody: "Tomorrow",
              links: [],
              receivedAt: "2026-09-01T00:00:00Z",
              attachments: [],
            },
          ]),
        );
        yield* Deferred.await(started);
        yield* Fiber.interrupt(fiber);
        return yield* Fiber.await(fiber);
      }),
    );

    expect(Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)).toBe(true);
    expect(mocks.record).not.toHaveBeenCalled();
  });

  it("replays a lost create acknowledgement against the same CalDAV resource", async () => {
    mocks.discover.mockResolvedValue({
      calendarUrl: "https://caldav.example/calendar/",
      authHeader: "Basic test",
    });
    mocks.extract.mockReturnValue(
      Effect.succeed({
        events: [
          {
            action: "create",
            title: "Dentist",
            startDate: "2026-09-03",
            allDay: true,
          },
        ],
        costCents: 0,
      }),
    );
    mocks.create.mockReturnValue(
      Effect.succeed({
        status: "success",
        eventUid: "omni-stable@omni-notify",
      }),
    );
    mocks.recordCreated.mockImplementationOnce(() => {
      throw new Error("crash after CalDAV accepted PUT");
    });
    const { CalendarEventPipeline } = await import("./pipeline.js");
    const pipeline = new CalendarEventPipeline(
      { downloadAttachment: vi.fn() } as never,
      new Logger("CalendarPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );
    const appointment = {
      id: "mail-replay",
      subject: "Appointment",
      from: "clinic@example.com",
      textBody: "Tomorrow",
      links: [],
      receivedAt: "2026-09-01T00:00:00Z",
      attachments: [],
    };

    await expect(
      Effect.runPromise(pipeline.handleEmailsEffect([appointment])),
    ).rejects.toThrow("crash after CalDAV accepted PUT");
    await Effect.runPromise(pipeline.handleEmailsEffect([appointment]));

    expect(mocks.create).toHaveBeenCalledTimes(2);
    expect(mocks.create.mock.calls.map((call) => call[3])).toEqual([
      "omni-stable@omni-notify",
      "omni-stable@omni-notify",
    ]);
  });
});
