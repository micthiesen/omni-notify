import { Docstore } from "@micthiesen/mitools/docstore";
import { Sqlite } from "@micthiesen/mitools/sqlite";
import { expect, layer } from "@effect/vitest";
import { Effect, Option } from "effect";
import {
  computeEventHash,
  CreatedCalendarEventEntity,
  type CreatedCalendarEventData,
  reconcileEventHashesEffect,
  replaceCreatedEventEffect,
} from "./persistence.js";

const event = (eventHash: string, title = "Dentist"): CreatedCalendarEventData => ({
  eventHash,
  emailId: "email-1",
  calendarEventId: "caldav-1",
  title,
  startDate: "2026-09-10",
  allDay: true,
  createdAt: 1_800_000_000_000,
});

const sqlLiteral = (value: string) => `'${value.replaceAll("'", "''")}'`;

layer(Docstore.layerMemory)("Calendar event transactions", (it) => {
  it.effect("rolls back the replacement when cancelling the prior row fails", () =>
    Effect.gen(function* () {
      yield* CreatedCalendarEventEntity.deleteAll();
      const prior = event("old-event");
      const replacement = event("new-event", "Dentist moved");
      yield* CreatedCalendarEventEntity.upsert(prior);
      const { db } = yield* Sqlite;
      const priorPk = CreatedCalendarEventEntity.getPk({
        eventHash: prior.eventHash,
      });
      yield* Effect.sync(() =>
        db.exec(
          `CREATE TRIGGER fail_calendar_cancel BEFORE UPDATE ON blobs ` +
            `WHEN NEW.pk = ${sqlLiteral(priorPk)} BEGIN SELECT RAISE(ABORT, 'forced'); END`,
        ),
      );

      const result = yield* Effect.result(
        replaceCreatedEventEffect(replacement, prior.eventHash),
      );
      yield* Effect.sync(() => db.exec("DROP TRIGGER fail_calendar_cancel"));

      expect(result._tag).toBe("Failure");
      expect(
        Option.isNone(
          yield* CreatedCalendarEventEntity.get({
            eventHash: replacement.eventHash,
          }),
        ),
      ).toBe(true);
      expect(
        Option.getOrUndefined(
          yield* CreatedCalendarEventEntity.get({ eventHash: prior.eventHash }),
        )?.status,
      ).toBeUndefined();
    }),
  );

  it.effect("rolls back a rekey when deleting the legacy row fails", () =>
    Effect.gen(function* () {
      yield* CreatedCalendarEventEntity.deleteAll();
      const prior = event("legacy-hash");
      const expected = computeEventHash(prior.title, prior.startDate, prior.startTime);
      yield* CreatedCalendarEventEntity.upsert(prior);
      const { db } = yield* Sqlite;
      const priorPk = CreatedCalendarEventEntity.getPk({
        eventHash: prior.eventHash,
      });
      yield* Effect.sync(() =>
        db.exec(
          `CREATE TRIGGER fail_calendar_rekey BEFORE DELETE ON blobs ` +
            `WHEN OLD.pk = ${sqlLiteral(priorPk)} BEGIN SELECT RAISE(ABORT, 'forced'); END`,
        ),
      );

      const result = yield* Effect.result(reconcileEventHashesEffect());
      yield* Effect.sync(() => db.exec("DROP TRIGGER fail_calendar_rekey"));

      expect(result._tag).toBe("Failure");
      expect(
        Option.isNone(yield* CreatedCalendarEventEntity.get({ eventHash: expected })),
      ).toBe(true);
      expect(
        Option.isSome(
          yield* CreatedCalendarEventEntity.get({ eventHash: prior.eventHash }),
        ),
      ).toBe(true);
    }),
  );
});
