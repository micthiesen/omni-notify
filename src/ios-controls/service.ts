import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Clock, Data, Effect, Schedule } from "effect";
import type { Streamer } from "../live-check/streamers.js";
import type { ApnsControlClient } from "./apns.js";
import {
  buildLiveControlSlots,
  IOS_CONTROL_SLOT_COUNT,
  liveControlSlotHash,
} from "./liveSlots.js";
import {
  type ApnsEnvironment,
  IOSControlPersistence,
  listIOSControlRegistrations,
} from "./persistence.js";

export type ControlRegistrationInput = {
  controlId: string;
  slot: number;
  pushToken: string;
  environment: ApnsEnvironment;
};

class TransientControlPushError extends Data.TaggedError("TransientControlPushError")<{
  readonly message: string;
}> {}

export class IOSControlService {
  private permanentFailures = new Set<string>();
  private readonly logger: NamedLogger;
  private lastReconciledAt: number | null = null;

  public constructor(
    private readonly streamers: Streamer[],
    private readonly homeUrl: string,
    parentLogger: NamedLogger,
    private readonly apns?: ApnsControlClient,
  ) {
    this.logger = parentLogger.extend("IOSControls");
  }

  public getSlotEffect(slot: number) {
    return Effect.gen({ self: this }, function* () {
      if (!Number.isInteger(slot) || slot < 1 || slot > IOS_CONTROL_SLOT_COUNT)
        return undefined;
      return (yield* buildLiveControlSlots(this.streamers, this.homeUrl))[slot - 1];
    });
  }

  public diagnosticsEffect() {
    return Effect.gen({ self: this }, function* () {
      const registrations = yield* listIOSControlRegistrations();
      const hashes = yield* this.desiredHashes();
      return {
        apnsEnabled: this.apns !== undefined,
        registrationCount: registrations.length,
        undeliveredCount: registrations.filter((row) => {
          const hash = hashes.get(row.slot);
          return hash !== undefined && row.lastDeliveredHash !== hash;
        }).length,
        lastReconciledAt: this.lastReconciledAt,
      };
    });
  }

  public registerDeviceEffect(deviceId: string, controls: ControlRegistrationInput[]) {
    return Effect.gen({ self: this }, function* () {
      const previous = new Map(
        (yield* IOSControlPersistence.list())
          .filter((row) => row.deviceId === deviceId)
          .map((row) => [row.registrationId, row]),
      );
      const rows = yield* IOSControlPersistence.replaceDevice(deviceId, controls);
      const currentIds = new Set(rows.map((row) => row.registrationId));
      for (const row of previous.values()) {
        if (!currentIds.has(row.registrationId)) {
          this.clearPermanentFailures(row.registrationId);
        }
      }
      if (!this.apns) return;
      for (const row of rows) {
        // An explicit app sync is also an operator-requested retry after fixing
        // a permanent APNs configuration error.
        this.clearPermanentFailures(row.registrationId);
      }
      yield* this.deliverEffect(yield* this.undeliveredRegistrationsEffect());
    });
  }

  public reconcileEffect() {
    return Effect.gen({ self: this }, function* () {
      const slots = yield* buildLiveControlSlots(this.streamers, this.homeUrl);
      this.lastReconciledAt = yield* Clock.currentTimeMillis;
      if (!this.apns) return;
      const hashes = new Map(
        slots.map((slot) => [slot.slot, liveControlSlotHash(slot)]),
      );
      yield* this.deliverEffect(yield* this.undeliveredRegistrationsEffect(hashes));
    });
  }

  public close(): void {
    this.apns?.close();
  }

  public readonly closeEffect = Effect.sync(() => this.close());

  private desiredHashes() {
    return buildLiveControlSlots(this.streamers, this.homeUrl).pipe(
      Effect.map(
        (slots) => new Map(slots.map((slot) => [slot.slot, liveControlSlotHash(slot)])),
      ),
    );
  }

  private undeliveredRegistrationsEffect(hashes?: Map<number, string>) {
    return Effect.gen({ self: this }, function* () {
      const desired = hashes ?? (yield* this.desiredHashes());
      const registrations = yield* IOSControlPersistence.list();
      return registrations.flatMap((row) => {
        const hash = desired.get(row.slot);
        if (!hash || row.lastDeliveredHash === hash) return [];
        return [{ registrationId: row.registrationId, hash }];
      });
    });
  }

  private deliverEffect(
    registrations: Array<{ registrationId: string; hash: string }>,
  ) {
    return Effect.forEach(
      registrations,
      ({ registrationId, hash }) =>
        Effect.gen({ self: this }, function* () {
          const failureKey = `${registrationId}:${hash}`;
          if (this.permanentFailures.has(failureKey)) return;
          const outcome = yield* this.pushEffect(registrationId, hash);
          if (outcome === "permanent") this.permanentFailures.add(failureKey);
          else if (outcome === "delivered") {
            this.clearPermanentFailures(registrationId);
          }
        }),
      { concurrency: "unbounded", discard: true },
    );
  }

  private clearPermanentFailures(registrationId: string): void {
    for (const key of this.permanentFailures) {
      if (key.startsWith(`${registrationId}:`)) this.permanentFailures.delete(key);
    }
  }

  private pushEffect(registrationId: string, desiredHash: string) {
    return Effect.gen({ self: this }, function* () {
      const registration = (yield* IOSControlPersistence.list()).find(
        (row) => row.registrationId === registrationId,
      );
      if (!registration || !this.apns) return "delivered" as const;
      const apns = this.apns;
      const attempt = Effect.gen({ self: this }, function* () {
        const result = yield* apns.sendControlChangedEffect(registration).pipe(
          Effect.mapError(
            (cause) =>
              new TransientControlPushError({
                message: cause.message,
              }),
          ),
        );
        if (result.kind === "sent") {
          yield* IOSControlPersistence.markDelivered(
            registration.registrationId,
            registration.pushToken,
            desiredHash,
          );
          return "delivered" as const;
        }
        if (result.kind === "invalid-token") {
          yield* IOSControlPersistence.delete(
            registration.registrationId,
            registration.pushToken,
          );
          yield* this.logger.info(`Removed stale control token: ${result.reason}`);
          return "delivered" as const;
        }
        const transient =
          result.status === 0 || result.status === 429 || result.status >= 500;
        if (transient) {
          return yield* new TransientControlPushError({
            message: `Control push failed (${result.status}): ${result.reason}`,
          });
        }
        yield* this.logger.warn(
          `Control push failed (${result.status}): ${result.reason}`,
        );
        return "permanent" as const;
      });
      return yield* attempt.pipe(
        Effect.retry({ schedule: Schedule.spaced("250 millis"), times: 1 }),
        Effect.catchTag("TransientControlPushError", (error) =>
          this.logger.warn(error.message).pipe(Effect.as("transient" as const)),
        ),
      );
    });
  }
}
