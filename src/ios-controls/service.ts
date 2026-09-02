import type { Effect as EffectType } from "effect/Effect";
import type { Logger } from "@micthiesen/mitools/logging";
import { Data, Effect, Schedule } from "effect";
import type { Streamer } from "../live-check/streamers.js";
import type { ApnsControlClient } from "./apns.js";
import {
  buildLiveControlSlots,
  IOS_CONTROL_SLOT_COUNT,
  type LiveControlSlot,
  liveControlSlotHash,
} from "./liveSlots.js";
import {
  type ApnsEnvironment,
  deleteIOSControlRegistration,
  listIOSControlRegistrations,
  markIOSControlDelivered,
  replaceDeviceRegistrations,
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
  private readonly logger: Logger;
  private lastReconciledAt: number | null = null;

  public constructor(
    private readonly streamers: Streamer[],
    private readonly homeUrl: string,
    parentLogger: Logger,
    private readonly apns?: ApnsControlClient,
  ) {
    this.logger = parentLogger.extend("IOSControls");
  }

  public getSlot(slot: number): LiveControlSlot | undefined {
    if (!Number.isInteger(slot) || slot < 1 || slot > IOS_CONTROL_SLOT_COUNT) {
      return undefined;
    }
    return buildLiveControlSlots(this.streamers, this.homeUrl)[slot - 1];
  }

  public diagnostics(): {
    apnsEnabled: boolean;
    registrationCount: number;
    undeliveredCount: number;
    lastReconciledAt: number | null;
  } {
    return {
      apnsEnabled: this.apns !== undefined,
      registrationCount: listIOSControlRegistrations().length,
      undeliveredCount: this.undeliveredRegistrations().length,
      lastReconciledAt: this.lastReconciledAt,
    };
  }

  public registerDeviceEffect(
    deviceId: string,
    controls: ControlRegistrationInput[],
  ): EffectType<void> {
    return Effect.gen({ self: this }, function* () {
      const previous = new Map(
        listIOSControlRegistrations()
          .filter((row) => row.deviceId === deviceId)
          .map((row) => [row.registrationId, row]),
      );
      const rows = replaceDeviceRegistrations(deviceId, controls);
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
      yield* this.deliverEffect(this.undeliveredRegistrations());
    });
  }

  public reconcileEffect(): EffectType<void> {
    return Effect.gen({ self: this }, function* () {
      const slots = buildLiveControlSlots(this.streamers, this.homeUrl);
      this.lastReconciledAt = Date.now();
      if (!this.apns) return;
      const hashes = new Map(
        slots.map((slot) => [slot.slot, liveControlSlotHash(slot)]),
      );
      yield* this.deliverEffect(this.undeliveredRegistrations(hashes));
    });
  }

  public close(): void {
    this.apns?.close();
  }

  private desiredHashes(): Map<number, string> {
    return new Map(
      buildLiveControlSlots(this.streamers, this.homeUrl).map((slot) => [
        slot.slot,
        liveControlSlotHash(slot),
      ]),
    );
  }

  private undeliveredRegistrations(
    hashes = this.desiredHashes(),
  ): Array<{ registrationId: string; hash: string }> {
    return listIOSControlRegistrations().flatMap((row) => {
      const hash = hashes.get(row.slot);
      if (!hash || row.lastDeliveredHash === hash) return [];
      return [{ registrationId: row.registrationId, hash }];
    });
  }

  private deliverEffect(
    registrations: Array<{ registrationId: string; hash: string }>,
  ): EffectType<void> {
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

  private pushEffect(
    registrationId: string,
    desiredHash: string,
  ): EffectType<"delivered" | "transient" | "permanent"> {
    const registration = listIOSControlRegistrations().find(
      (row) => row.registrationId === registrationId,
    );
    if (!registration || !this.apns) return Effect.succeed("delivered");
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
        markIOSControlDelivered(
          registration.registrationId,
          registration.pushToken,
          desiredHash,
        );
        return "delivered" as const;
      }
      if (result.kind === "invalid-token") {
        deleteIOSControlRegistration(
          registration.registrationId,
          registration.pushToken,
        );
        this.logger.info(`Removed stale control token: ${result.reason}`);
        return "delivered" as const;
      }
      const transient =
        result.status === 0 || result.status === 429 || result.status >= 500;
      if (transient) {
        return yield* new TransientControlPushError({
          message: `Control push failed (${result.status}): ${result.reason}`,
        });
      }
      this.logger.warn(`Control push failed (${result.status}): ${result.reason}`);
      return "permanent" as const;
    });
    return attempt.pipe(
      Effect.retry({ schedule: Schedule.spaced("250 millis"), times: 1 }),
      Effect.catchTag("TransientControlPushError", (error) =>
        Effect.sync(() => {
          this.logger.warn(error.message);
          return "transient" as const;
        }),
      ),
    );
  }
}
