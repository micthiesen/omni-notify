import { Logger } from "@micthiesen/mitools/logging";
import { runTest, testRuntime } from "../live-check/testRuntime.js";
import { Deferred, Effect, Fiber } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  StreamerStatusEntity,
  upsertStreamerStatusEffect,
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

afterEach(async () => {
  await runTest(StreamerStatusEntity.deleteAll());
  await runTest(IOSControlRegistrationEntity.deleteAll());
});

describe("IOSControlService", () => {
  it("pushes a new token once but not on an unchanged app resync", async () => {
    const sendControlChanged = vi.fn().mockResolvedValue({ kind: "sent" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
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

    await runTest(service.registerDeviceEffect("device-one", controls));
    await runTest(service.registerDeviceEffect("device-one", controls));

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
  });

  it("persists delivery hashes across service restarts", async () => {
    const firstSend = vi.fn().mockResolvedValue({ kind: "sent" });
    const first = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      mockApns(firstSend),
    );
    await runTest(
      first.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );
    expect(firstSend).toHaveBeenCalledTimes(1);

    const restartedSend = vi.fn().mockResolvedValue({ kind: "sent" });
    const restarted = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      mockApns(restartedSend),
    );
    await runTest(restarted.reconcileEffect());

    expect(restartedSend).not.toHaveBeenCalled();
    expect((await runTest(restarted.diagnosticsEffect())).undeliveredCount).toBe(0);
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
      Logger.named("Test"),
      apns,
    );

    await runTest(
      service.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );

    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    expect((await runTest(service.diagnosticsEffect())).undeliveredCount).toBe(0);
  });

  it("carries an exhausted transient push into later live-check ticks", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValue({ kind: "failed", status: 503, reason: "Shutdown" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      apns,
    );

    await runTest(
      service.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );
    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    expect((await runTest(service.diagnosticsEffect())).undeliveredCount).toBe(1);

    await runTest(service.reconcileEffect());
    expect(sendControlChanged).toHaveBeenCalledTimes(4);
    expect((await runTest(service.diagnosticsEffect())).undeliveredCount).toBe(1);

    sendControlChanged.mockResolvedValue({ kind: "sent" });
    await runTest(service.reconcileEffect());
    expect(sendControlChanged).toHaveBeenCalledTimes(5);
    expect((await runTest(service.diagnosticsEffect())).undeliveredCount).toBe(0);
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
      Logger.named("Test"),
      apns,
    );

    await runTest(
      service.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );

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
      Logger.named("Test"),
      apns,
    );

    await runTest(
      service.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
    expect(await runTest(listIOSControlRegistrations())).toEqual([]);
  });

  it("does not retry a permanent APNs rejection", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValue({ kind: "failed", status: 403, reason: "Forbidden" });
    const apns = mockApns(sendControlChanged);
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      apns,
    );

    await runTest(
      service.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
    expect(await runTest(listIOSControlRegistrations())).toHaveLength(1);
  });

  it("retries a missing APNs status as a transient transport failure", async () => {
    const sendControlChanged = vi
      .fn()
      .mockResolvedValueOnce({ kind: "failed", status: 0, reason: "No response" })
      .mockResolvedValueOnce({ kind: "sent" });
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      mockApns(sendControlChanged),
    );

    await runTest(
      service.registerDeviceEffect("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );

    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    expect((await runTest(service.diagnosticsEffect())).undeliveredCount).toBe(0);
  });

  it("pushes only registered slots whose displayed state changed", async () => {
    const sendControlChanged = vi.fn().mockResolvedValue({ kind: "sent" });
    const apns = mockApns(sendControlChanged);
    await runTest(
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
      ]),
    );
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      apns,
    );
    await runTest(service.reconcileEffect());
    // Rows restored after a process restart are reconciled once even before a
    // new state transition, then their delivered hashes suppress duplicates.
    expect(sendControlChanged).toHaveBeenCalledTimes(2);
    sendControlChanged.mockClear();

    await runTest(
      upsertStreamerStatusEffect({
        streamerId: "alpha",
        isLive: true,
        primary: { platform: Platform.Twitch, username: "alpha" },
        primaryTitle: "Alpha is live",
        startedAt: new Date(),
        maxViewerCount: 12,
        viewerCount: 10,
      }),
    );
    await runTest(service.reconcileEffect());

    expect(sendControlChanged).toHaveBeenCalledTimes(1);
    expect(sendControlChanged.mock.calls[0][0]).toMatchObject({ slot: 1 });
  });

  it("interrupts an in-flight APNs request with its parent reconciliation", async () => {
    const started = await Effect.runPromise(Deferred.make<void>());
    const cancelled = await Effect.runPromise(Deferred.make<void>());
    const service = new IOSControlService(
      [streamer],
      "http://omni.boris",
      Logger.named("Test"),
      {
        sendControlChangedEffect: () =>
          Deferred.succeed(started, undefined).pipe(
            Effect.andThen(Effect.never),
            Effect.ensuring(Deferred.succeed(cancelled, undefined)),
          ),
        close: vi.fn(),
      } as unknown as ApnsControlClient,
    );
    await runTest(
      replaceDeviceRegistrations("device-one", [
        {
          controlId: "slot-one",
          slot: 1,
          pushToken: "ab".repeat(32),
          environment: "sandbox",
        },
      ]),
    );

    const fiber = testRuntime.runFork(service.reconcileEffect());
    await Effect.runPromise(Deferred.await(started));
    await Effect.runPromise(Fiber.interrupt(fiber));

    await Effect.runPromise(Deferred.await(cancelled));
    expect((await runTest(service.diagnosticsEffect())).undeliveredCount).toBe(1);
  });
});
