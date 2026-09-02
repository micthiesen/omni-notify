import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Clock, Effect, Option, Schema } from "effect";
import { PersistenceError } from "../effect/errors.js";

export type ApnsEnvironment = "sandbox" | "production";
export type IOSControlRegistration = {
  registrationId: string;
  deviceId: string;
  controlId: string;
  slot: number;
  pushToken: string;
  environment: ApnsEnvironment;
  lastDeliveredHash?: string;
  createdAt: number;
  updatedAt: number;
};

export const IOSControlRegistrationEntity = new Entity<
  IOSControlRegistration,
  ["registrationId"]
>("ios-control-registration", ["registrationId"]);
const IOSControlRegistrationSchema = Schema.Struct({
  registrationId: Schema.String,
  deviceId: Schema.String,
  controlId: Schema.String,
  slot: Schema.Number,
  pushToken: Schema.String,
  environment: Schema.Literals(["sandbox", "production"]),
  lastDeliveredHash: Schema.optional(Schema.String),
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
});
const fail = (operation: string) => (cause: unknown) =>
  new PersistenceError({ operation, cause });

export function listIOSControlRegistrations() {
  return IOSControlRegistrationEntity.getAll().pipe(
    Effect.flatMap((rows) =>
      Effect.forEach(rows, (row) =>
        Schema.decodeUnknownEffect(IOSControlRegistrationSchema)(row),
      ),
    ),
    Effect.mapError(fail("read iOS control registrations")),
  );
}

export function replaceDeviceRegistrations(
  deviceId: string,
  controls: Array<
    Pick<IOSControlRegistration, "controlId" | "slot" | "pushToken" | "environment">
  >,
) {
  return Effect.gen(function* () {
    const now = yield* Clock.currentTimeMillis;
    const docstore = yield* Docstore;
    return yield* docstore.transaction("replace iOS control registrations", (tx) => {
      const existing = tx
        .getRawRowsByPrefix(`$${IOSControlRegistrationEntity.name}#`)
        .map((raw) =>
          Schema.decodeUnknownSync(IOSControlRegistrationSchema)(decodeDoc(raw.data)),
        )
        .filter((row) => row.deviceId === deviceId);
      const incomingIds = new Set(
        controls.map((control) => `${deviceId}:${control.controlId}`),
      );
      for (const row of existing) {
        if (!incomingIds.has(row.registrationId)) {
          tx.deleteDoc(
            IOSControlRegistrationEntity.getPk({
              registrationId: row.registrationId,
            }),
          );
        }
      }
      return controls.map((control) => {
        const registrationId = `${deviceId}:${control.controlId}`;
        const pk = IOSControlRegistrationEntity.getPk({ registrationId });
        const previousRaw = tx.getRawRow(pk, now);
        const previous = previousRaw
          ? Schema.decodeUnknownSync(IOSControlRegistrationSchema)(
              decodeDoc(previousRaw.data),
            )
          : undefined;
        const unchanged =
          previous?.slot === control.slot &&
          previous.pushToken === control.pushToken &&
          previous.environment === control.environment;
        const row: IOSControlRegistration = {
          registrationId,
          deviceId,
          ...control,
          lastDeliveredHash: unchanged ? previous.lastDeliveredHash : undefined,
          createdAt: previous?.createdAt ?? now,
          updatedAt: now,
        };
        tx.upsertDoc(pk, row, { entity: IOSControlRegistrationEntity.name }, now);
        return row;
      });
    });
  }).pipe(Effect.mapError(fail("replace iOS control registrations")));
}

export function markIOSControlDelivered(
  registrationId: string,
  pushToken: string,
  hash: string,
) {
  return IOSControlRegistrationEntity.update({ registrationId }, (current) =>
    current.pushToken === pushToken ? { ...current, lastDeliveredHash: hash } : current,
  ).pipe(Effect.asVoid, Effect.mapError(fail("mark iOS control delivered")));
}

export function deleteIOSControlRegistration(
  registrationId: string,
  pushToken: string,
) {
  return Effect.gen(function* () {
    const current = Option.getOrUndefined(
      yield* IOSControlRegistrationEntity.get({ registrationId }),
    );
    if (current?.pushToken === pushToken)
      yield* IOSControlRegistrationEntity.delete({ registrationId });
  }).pipe(Effect.mapError(fail("delete iOS control registration")));
}

export const IOSControlPersistence = {
  list: Effect.fn("IOSControls.listRegistrations")(listIOSControlRegistrations),
  replaceDevice: Effect.fn("IOSControls.replaceDevice")(replaceDeviceRegistrations),
  markDelivered: Effect.fn("IOSControls.markDelivered")(markIOSControlDelivered),
  delete: Effect.fn("IOSControls.deleteRegistration")(deleteIOSControlRegistration),
} as const;
