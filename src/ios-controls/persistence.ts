import { Entity } from "@micthiesen/mitools/entities";
import { transaction } from "@micthiesen/mitools/docstore";
import { Clock, Effect, Schema } from "effect";
import { fromSync } from "../effect/interop.js";

export type ApnsEnvironment = "sandbox" | "production";

export type IOSControlRegistration = {
  registrationId: string;
  deviceId: string;
  controlId: string;
  slot: number;
  pushToken: string;
  environment: ApnsEnvironment;
  /** Hash of the slot state APNs last successfully prompted this control to fetch. */
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

export function listIOSControlRegistrations(): IOSControlRegistration[] {
  return IOSControlRegistrationEntity.getAll();
}

export function replaceDeviceRegistrations(
  deviceId: string,
  controls: Array<
    Pick<IOSControlRegistration, "controlId" | "slot" | "pushToken" | "environment">
  >,
  now = Date.now(),
): IOSControlRegistration[] {
  return transaction(() => {
    const existing = listIOSControlRegistrations().filter(
      (r) => r.deviceId === deviceId,
    );
    const incomingIds = new Set(
      controls.map((control) => `${deviceId}:${control.controlId}`),
    );
    for (const row of existing) {
      if (!incomingIds.has(row.registrationId)) {
        IOSControlRegistrationEntity.delete({ registrationId: row.registrationId });
      }
    }

    return controls.map((control) => {
      const registrationId = `${deviceId}:${control.controlId}`;
      const previous = IOSControlRegistrationEntity.get({ registrationId });
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
      IOSControlRegistrationEntity.upsert(row);
      return row;
    });
  });
}

export function markIOSControlDelivered(
  registrationId: string,
  pushToken: string,
  hash: string,
): void {
  const current = IOSControlRegistrationEntity.get({ registrationId });
  // A concurrent re-registration may have rotated the token while this push
  // was in flight. Never mark that new token delivered from the old result.
  if (!current || current.pushToken !== pushToken) return;
  IOSControlRegistrationEntity.upsert({ ...current, lastDeliveredHash: hash });
}

export function deleteIOSControlRegistration(
  registrationId: string,
  pushToken: string,
): void {
  const current = IOSControlRegistrationEntity.get({ registrationId });
  // APNs may reject an old token while the app is concurrently registering
  // its replacement. Never let that stale response delete the new token.
  if (!current || current.pushToken !== pushToken) return;
  IOSControlRegistrationEntity.delete({ registrationId });
}

const decodeRegistration = (row: unknown) =>
  fromSync("decode iOS control registration", () =>
    Schema.decodeUnknownSync(IOSControlRegistrationSchema)(row),
  );

export const IOSControlPersistence = {
  list: Effect.fn("IOSControls.listRegistrations")(function* () {
    const rows = yield* fromSync("read iOS control registrations", () =>
      IOSControlRegistrationEntity.getAll(),
    );
    return yield* Effect.forEach(rows, decodeRegistration);
  }),
  replaceDevice: Effect.fn("IOSControls.replaceDevice")(function* (
    deviceId: string,
    controls: Array<
      Pick<IOSControlRegistration, "controlId" | "slot" | "pushToken" | "environment">
    >,
  ) {
    const now = yield* Clock.currentTimeMillis;
    return yield* fromSync("replace iOS control registrations", () =>
      replaceDeviceRegistrations(deviceId, controls, now),
    );
  }),
  markDelivered: Effect.fn("IOSControls.markDelivered")(function* (
    registrationId: string,
    pushToken: string,
    hash: string,
  ) {
    yield* fromSync("mark iOS control delivered", () =>
      markIOSControlDelivered(registrationId, pushToken, hash),
    );
  }),
  delete: Effect.fn("IOSControls.deleteRegistration")(function* (
    registrationId: string,
    pushToken: string,
  ) {
    yield* fromSync("delete iOS control registration", () =>
      deleteIOSControlRegistration(registrationId, pushToken),
    );
  }),
} as const;
