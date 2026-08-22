import type { Logger } from "@micthiesen/mitools/logging";
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

  public async registerDevice(
    deviceId: string,
    controls: ControlRegistrationInput[],
  ): Promise<void> {
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
    await this.deliver(this.undeliveredRegistrations());
  }

  public async reconcile(): Promise<void> {
    const slots = buildLiveControlSlots(this.streamers, this.homeUrl);
    this.lastReconciledAt = Date.now();
    if (!this.apns) return;
    const hashes = new Map(slots.map((slot) => [slot.slot, liveControlSlotHash(slot)]));
    await this.deliver(this.undeliveredRegistrations(hashes));
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

  private async deliver(
    registrations: Array<{ registrationId: string; hash: string }>,
  ): Promise<void> {
    await Promise.all(
      registrations.map(async ({ registrationId, hash }) => {
        const failureKey = `${registrationId}:${hash}`;
        if (this.permanentFailures.has(failureKey)) return;
        const outcome = await this.push(registrationId, hash);
        if (outcome === "permanent") this.permanentFailures.add(failureKey);
        else if (outcome === "delivered") {
          this.clearPermanentFailures(registrationId);
        }
      }),
    );
  }

  private clearPermanentFailures(registrationId: string): void {
    for (const key of this.permanentFailures) {
      if (key.startsWith(`${registrationId}:`)) this.permanentFailures.delete(key);
    }
  }

  private async push(
    registrationId: string,
    desiredHash: string,
  ): Promise<"delivered" | "transient" | "permanent"> {
    const registration = listIOSControlRegistrations().find(
      (row) => row.registrationId === registrationId,
    );
    if (!registration || !this.apns) return "delivered";
    for (let attempt = 1; attempt <= 2; attempt++) {
      try {
        const result = await this.apns.sendControlChanged(registration);
        if (result.kind === "sent") {
          markIOSControlDelivered(
            registration.registrationId,
            registration.pushToken,
            desiredHash,
          );
          return "delivered";
        }
        if (result.kind === "invalid-token") {
          deleteIOSControlRegistration(
            registration.registrationId,
            registration.pushToken,
          );
          this.logger.info(`Removed stale control token: ${result.reason}`);
          return "delivered";
        }
        const transient =
          result.status === 0 || result.status === 429 || result.status >= 500;
        if (transient && attempt === 1) {
          await new Promise((resolve) => setTimeout(resolve, 250));
          continue;
        }
        this.logger.warn(`Control push failed (${result.status}): ${result.reason}`);
        return transient ? "transient" : "permanent";
      } catch (error) {
        if (attempt === 1) {
          await new Promise((resolve) => setTimeout(resolve, 250));
          continue;
        }
        this.logger.warn(`Control push failed: ${(error as Error).message}`);
        return "transient";
      }
    }
    return "delivered";
  }
}
