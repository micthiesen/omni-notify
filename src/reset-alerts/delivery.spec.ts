import { Pushover } from "@micthiesen/mitools/pushover";
import { Effect, Layer } from "effect";
import { afterAll, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import { ResetDeliveryEntity as LegacyCodexEntity } from "../codex-resets/delivery.js";
import { createResetDelivery, type ResetAlert } from "./delivery.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

it("isolates providers while honoring persisted Codex reservations", async () => {
  const now = Date.now();
  const alert: ResetAlert = {
    key: "same-event",
    aliases: ["same-source-post"],
    title: "Reset report",
    message: "Reported reset",
    url: "https://example.com/source",
    occurredAt: now,
  };
  await runtime.run(
    LegacyCodexEntity.upsert({
      key: alert.key,
      status: "sent",
      occurredAt: now,
      updatedAt: now,
    }),
  );
  const send = vi.fn(() => Effect.void);
  const pushover = Layer.succeed(
    Pushover,
    Pushover.of({ enabled: true, notify: send }),
  );
  const codex = createResetDelivery("codex");
  const claude = createResetDelivery("claude");
  const run = (delivery: typeof codex) =>
    runtime.run(
      delivery.deliverResetAlerts([alert], now).pipe(Effect.provide(pushover)),
    );
  expect(await run(codex)).toEqual({ sent: 0, skipped: 1, uncertain: 0 });
  expect(await run(claude)).toEqual({ sent: 1, skipped: 0, uncertain: 0 });
  expect(await run(createResetDelivery("claude"))).toEqual({
    sent: 0,
    skipped: 1,
    uncertain: 0,
  });
  expect(send).toHaveBeenCalledTimes(1);
  expect(codex.ResetDeliveryEntity.name).toBe("codex-reset-delivery");
  expect(claude.ResetDeliveryEntity.name).toBe("claude-reset-delivery");
});
