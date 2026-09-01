import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { Deferred, Effect, Fiber } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  StreamerStatusEntity,
  upsertStreamerStatus,
} from "../live-check/persistence.js";
import { Platform } from "../live-check/platforms/index.js";
import type { Streamer } from "../live-check/streamers.js";
import {
  type ApnsControlClient,
  type ApnsControlPushResult,
  ApnsTransportError,
} from "./apns.js";
import {
  type IOSControlRegistration,
  IOSControlRegistrationEntity,
  listIOSControlRegistrations,
  replaceDeviceRegistrations,
} from "./persistence.js";
import { IOSControlService } from "./service.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "ios-control-service.spec.db",
  },
});

const streamer: Streamer = {
  id: "alpha",
  displayName: "Alpha",
  bindings: [{ platform: Platform.Twitch, username: "alpha" }],
  tier: "primary",
};

function mockApns(
  sendControlChanged: (
    registration: IOSControlRegistration,
  ) => Promise<ApnsControlPushResult>,
): ApnsControlClient {
  return {
    sendControlChangedEffect: (registration: IOSControlRegistration) =>
      Effect.tryPromise({
        try: () => sendControlChanged(registration),
        catch: (cause) => new ApnsTransportError({ cause }),
      }),
    close: vi.fn(),
  } as unknown as ApnsControlClient;
}

afterEach(() => {
  StreamerStatusEntity.deleteAll();
  IOSControlRegistrationEntity.deleteAll();
});

describe("IOSControlService", () => {
  it("pushes a new token once but not on an unchanged app resync", async () => {
    const sendControlChanged = vi.fn().mockResolvedValue({ kind: "sent" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );
    const controls = [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox" as const,
      },
    ];

    await service.registerDevice("device-one", controls);
    await service.registerDevice("device-one", controls);

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
  });

  it("persists delivery hashes across service restarts", async () => {
    const firstSend = vi.fn().mockResolvedValue({ kind: "sent" });
    const first = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      mockApns(firstSend),
    );
    await first.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);
    expect(firstSend).toHaveBeenCalledTimes(1);

    const restartedSend = vi.fn().mockResolvedValue({ kind: "sent" });
    const restarted = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      mockApns(restartedSend),
    );
    await restarted.reconcile();

    expect(restartedSend).not.toHaveBeenCalled();
    expect(restarted.diagnostics().undeliveredCount).toBe(0);
  });

  it("retries one transient APNs response", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValueOnce({ kind: "failed", status: 503, reason: "Shutdown" })
      .mockResolvedValueOnce({ kind: "sent" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );

    await service.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);

    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    expect(service.diagnostics().undeliveredCount).toBe(0);
  });

  it("carries an exhausted transient push into later live-check ticks", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValue({ kind: "failed", status: 503, reason: "Shutdown" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );

    await service.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);
    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    expect(service.diagnostics().undeliveredCount).toBe(1);

    await service.reconcile();
    expect(sendControlChanged).toHaveBeenCalledTimes(4);
    expect(service.diagnostics().undeliveredCount).toBe(1);

    sendControlChanged.mockResolvedValue({ kind: "sent" });
    await service.reconcile();
    expect(sendControlChanged).toHaveBeenCalledTimes(5);
    expect(service.diagnostics().undeliveredCount).toBe(0);
  });

  it("retries one transport error", async () => {
    const sendControlChanged = vi
      .fn()
      .mockRejectedValueOnce(new Error("socket reset"))
      .mockResolvedValueOnce({ kind: "sent" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );

    await service.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);

    expect(sendControlChanged).toHaveBeenCalledTimes(2);
  });

  it("deletes a token Apple reports as invalid", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValue({ kind: "invalid-token", reason: "Unregistered" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );

    await service.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
    expect(listIOSControlRegistrations()).toEqual([]);
  });

  it("does not retry a permanent APNs rejection", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValue({ kind: "failed", status: 403, reason: "Forbidden" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );

    await service.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
    expect(listIOSControlRegistrations()).toHaveLength(1);
  });

  it("retries a missing APNs status as a transient transport failure", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValueOnce({ kind: "failed", status: 0, reason: "No response" })
      .mockResolvedValueOnce({ kind: "sent" });
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      mockApns(sendControlChanged),
    );

    await service.registerDevice("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);

    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    expect(service.diagnostics().undeliveredCount).toBe(0);
  });

  it("pushes only registered slots whose displayed state changed", async () => {
    const sendControlChanged = vi.fn().mockResolvedValue({ kind: "sent" });
    const apns = mockApns(sendControlChanged);
    replaceDeviceRegistrations("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
      {
        controlId: "slot-four",
        slot: 4,
        pushToken: "cd".repeat(32),
        environment: "sandbox",
      },
    ]);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      apns,
    );
    await service.reconcile();
    // Rows restored after a process restart are reconciled once even before a
    // new state transition, then their delivered hashes suppress duplicates.
    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    sendControlChanged.mockClear();

    upsertStreamerStatus({
      streamerId: "alpha",
      isLive: true,
      primary: { platform: Platform.Twitch, username: "alpha" },
      primaryTitle: "Alpha is live",
      startedAt: new Date(),
      maxViewerCount: 12,
      viewerCount: 10,
    });
    await service.reconcile();

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
    expect(sendControlChanged.mock.calls[0][0]).toMatchObject({ slot: 1 });
  });

  it("interrupts an in-flight APNs request with its parent reconciliation", async () => {
    const started = await Effect.runPromise(Deferred.make<void>());
    const cancelled = await Effect.runPromise(Deferred.make<void>());
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      new Logger("Test"),
      {
        sendControlChangedEffect: () =>
          Deferred.succeed(started, undefined).pipe(
            Effect.andThen(Effect.never),
            Effect.ensuring(Deferred.succeed(cancelled, undefined)),
          ),
        close: vi.fn(),
      } as unknown as ApnsControlClient,
    );
    replaceDeviceRegistrations("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "ab".repeat(32),
        environment: "sandbox",
      },
    ]);

    const fiber = Effect.runFork(service.reconcileEffect());
    await Effect.runPromise(Deferred.await(started));
    await Effect.runPromise(Fiber.interrupt(fiber));

    await Effect.runPromise(Deferred.await(cancelled));
    expect(service.diagnostics().undeliveredCount).toBe(1);
  });
});
