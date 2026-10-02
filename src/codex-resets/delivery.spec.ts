import {
  Pushover,
  PushoverError,
  type PushoverMessage,
} from "@micthiesen/mitools/pushover";
import { Effect, Layer } from "effect";
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import {
  deliverResetAlerts,
  ResetDeliveryEntity,
  type ResetAlert,
} from "./delivery.js";

const alert: ResetAlert = {
  key: "session:codex-2026-10-02T12:00:00Z",
  title: "Codex allowance reset",
  message: "The Codex usage allowance reset.",
  url: "http://omni.boris/tasks",
  occurredAt: 1_790_940_000_000,
};
const withKey = (key: string): ResetAlert => ({ ...alert, key });
const runtime = createMitoolsTestRuntime();
const send = vi.fn<(message: PushoverMessage) => Effect.Effect<void, PushoverError>>();
const pushover = Layer.succeed(Pushover, Pushover.of({ enabled: true, notify: send }));
const run = (alerts: readonly ResetAlert[], now: number) =>
  runtime.run(deliverResetAlerts(alerts, now).pipe(Effect.provide(pushover)));
const now = () => Date.now();

afterAll(() => runtime.dispose());
beforeEach(() => send.mockReset().mockReturnValue(Effect.void));

describe("deliverResetAlerts", () => {
  it("sends once and persists sent state across later runs", async () => {
    await expect(run([alert], now())).resolves.toEqual({
      sent: 1,
      skipped: 0,
      uncertain: 0,
    });
    await expect(run([alert], now())).resolves.toEqual({
      sent: 0,
      skipped: 1,
      uncertain: 0,
    });
    expect(send).toHaveBeenCalledTimes(1);
    await expect(
      runtime.run(ResetDeliveryEntity.get({ key: alert.key })),
    ).resolves.toMatchObject({
      _tag: "Some",
      value: { status: "sent" },
    });
  });

  it("atomically reserves duplicate concurrent deliveries", async () => {
    const concurrentAlert = withKey("concurrent-alert");
    let release!: () => void;
    send.mockReturnValue(
      Effect.promise(
        () =>
          new Promise<void>((resolve) => {
            release = resolve;
          }),
      ),
    );
    const first = run([concurrentAlert], now());
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    await expect(run([concurrentAlert], now())).resolves.toEqual({
      sent: 0,
      skipped: 0,
      uncertain: 1,
    });
    release();
    await expect(first).resolves.toEqual({ sent: 1, skipped: 0, uncertain: 0 });
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("retains ambiguous attempts after failure so a restarted run cannot resend", async () => {
    const uncertainAlert = withKey("uncertain-alert");
    send.mockReturnValueOnce(
      Effect.fail(
        new PushoverError({
          status: undefined,
          body: undefined,
          cause: "socket closed",
        }),
      ),
    );
    await expect(run([uncertainAlert], now())).rejects.toMatchObject({
      _tag: "ResetAlertDeliveryError",
      uncertain: true,
    });
    await expect(run([uncertainAlert], now())).resolves.toEqual({
      sent: 0,
      skipped: 0,
      uncertain: 1,
    });
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("releases a definite provider rejection for retry on the next poll", async () => {
    const rejectedAlert = withKey("rejected-alert");
    send.mockReturnValueOnce(
      Effect.fail(
        new PushoverError({ status: 429, body: "rate limited", cause: "rejected" }),
      ),
    );
    await expect(run([rejectedAlert], now())).rejects.toMatchObject({
      _tag: "ResetAlertDeliveryError",
      uncertain: false,
    });
    await expect(run([rejectedAlert], now())).resolves.toEqual({
      sent: 1,
      skipped: 0,
      uncertain: 0,
    });
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("fails clearly when the Pushover service is disabled", async () => {
    const disabled = Layer.succeed(
      Pushover,
      Pushover.of({ enabled: false, notify: () => Effect.void }),
    );
    await expect(
      runtime.run(
        deliverResetAlerts([withKey("disabled-alert")], now()).pipe(
          Effect.provide(disabled),
        ),
      ),
    ).rejects.toMatchObject({
      _tag: "ResetAlertDeliveryError",
      uncertain: false,
      cause: expect.objectContaining({
        message: expect.stringContaining("Pushover is disabled"),
      }),
    });
    expect(send).not.toHaveBeenCalled();
  });
});
