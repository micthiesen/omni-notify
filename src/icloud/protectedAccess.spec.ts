import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber, Result } from "effect";
import { TestClock } from "effect/testing";
import { vi } from "vitest";
import { ICloudPcsError, requestProtectedAccess } from "./protectedAccess.js";

const approved = { isICDRSDisabled: true, isDeviceConsentedForPCS: true };
const pending = {
  status: "pending",
  message: "Requested the device to upload cookies.",
};
function fixture(responses: Array<{ status: number; data: unknown }>) {
  let index = 0;
  return vi.fn((_operation: string, _endpoint: string, _body?: unknown) =>
    Effect.sync(() => {
      const response = responses[index++];
      if (!response) throw new Error("Unexpected request");
      return response;
    }),
  );
}
const ok = (data: unknown) => ({ status: 200, data });

describe("iCloud protected access", () => {
  for (const state of [{ isICDRSDisabled: false }]) {
    it.effect(`skips PCS when ICDRS is enabled: ${JSON.stringify(state)}`, () =>
      Effect.gen(function* () {
        const request = fixture([ok(state)]);
        expect(yield* requestProtectedAccess(request, "reminders")).toBe(
          "not-required",
        );
        expect(request).toHaveBeenCalledExactlyOnceWith(
          "PCS state",
          "requestWebAccessState",
          undefined,
        );
      }),
    );
  }

  for (const consent of [false, undefined]) {
    it.effect(`requires device approval when consent is ${consent}`, () =>
      Effect.gen(function* () {
        const state =
          consent === undefined
            ? { isICDRSDisabled: true }
            : {
                isICDRSDisabled: true,
                isDeviceConsentedForPCS: consent,
              };
        const request = fixture([
          ok(state),
          ok({ isDeviceConsentNotificationSent: true }),
        ]);
        expect(yield* requestProtectedAccess(request, "reminders")).toBe(
          "consent-required",
        );
        expect(request).toHaveBeenCalledTimes(2);
        expect(request).toHaveBeenLastCalledWith(
          "PCS consent",
          "enableDeviceConsentForPCS",
          undefined,
        );
      }),
    );
  }

  it.effect(
    "polls known pending states and marks only the first request as user initiated",
    () =>
      Effect.gen(function* () {
        const request = fixture([
          ok(approved),
          ok(pending),
          ok({ status: "pending", message: "Cookies not available yet on server." }),
          ok({ status: "success" }),
        ]);
        const fiber = yield* Effect.forkChild(
          requestProtectedAccess(request, "future-items"),
        );
        yield* TestClock.adjust("10 seconds");
        expect(yield* Fiber.join(fiber)).toBe("ready");
        expect(request.mock.calls.slice(1)).toEqual([
          [
            "PCS cookies",
            "requestPCS",
            { appName: "future-items", derivedFromUserAction: true },
          ],
          [
            "PCS cookies",
            "requestPCS",
            { appName: "future-items", derivedFromUserAction: false },
          ],
          [
            "PCS cookies",
            "requestPCS",
            { appName: "future-items", derivedFromUserAction: false },
          ],
        ]);
      }),
  );

  it.effect("stops after ten cookie attempts", () =>
    Effect.gen(function* () {
      const request = fixture([
        ok(approved),
        ...Array.from({ length: 10 }, () => ok(pending)),
      ]);
      const fiber = yield* Effect.forkChild(
        requestProtectedAccess(request, "reminders"),
      );
      yield* TestClock.adjust("45 seconds");
      expect(yield* Fiber.join(fiber)).toBe("consent-required");
      expect(request).toHaveBeenCalledTimes(11);
      yield* TestClock.adjust("1 minute");
      expect(request).toHaveBeenCalledTimes(11);
    }),
  );

  it.effect("interruption cancels polling", () =>
    Effect.gen(function* () {
      const request = fixture([ok(approved), ok(pending)]);
      const fiber = yield* Effect.forkChild(
        requestProtectedAccess(request, "reminders"),
      );
      yield* TestClock.adjust("1 second");
      expect(request).toHaveBeenCalledTimes(2);
      yield* Fiber.interrupt(fiber);
      yield* TestClock.adjust("1 minute");
      expect(request).toHaveBeenCalledTimes(2);
    }),
  );

  const failures = [
    {
      responses: [ok({})],
      operation: "PCS state",
      status: 200,
      reason: "invalid-response",
    },
    {
      responses: [{ status: 503, data: "secret" }],
      operation: "PCS state",
      status: 503,
      reason: "http",
    },
    {
      responses: [ok({ isICDRSDisabled: "false" })],
      operation: "PCS state",
      status: 200,
      reason: "invalid-response",
    },
    {
      responses: [ok({ isICDRSDisabled: false, isDeviceConsentedForPCS: "secret" })],
      operation: "PCS state",
      status: 200,
      reason: "invalid-response",
    },
    {
      responses: [ok(null)],
      operation: "PCS state",
      status: 200,
      reason: "invalid-response",
    },
    {
      responses: [ok({ isICDRSDisabled: true }), { status: 403, data: "secret" }],
      operation: "PCS consent",
      status: 403,
      reason: "http",
    },
    {
      responses: [
        ok({ isICDRSDisabled: true }),
        ok({ isDeviceConsentNotificationSent: false }),
      ],
      operation: "PCS consent",
      status: 200,
      reason: "consent-not-sent",
    },
    {
      responses: [ok({ isICDRSDisabled: true }), ok({})],
      operation: "PCS consent",
      status: 200,
      reason: "invalid-response",
    },
    {
      responses: [ok(approved), { status: 429, data: "secret" }],
      operation: "PCS cookies",
      status: 429,
      reason: "http",
    },
    {
      responses: [ok(approved), ok({ status: "failure", message: "secret" })],
      operation: "PCS cookies",
      status: 200,
      reason: "unknown-state",
    },
    {
      responses: [ok(approved), ok({ status: "failure" })],
      operation: "PCS cookies",
      status: 200,
      reason: "unknown-state",
    },
    {
      responses: [ok(approved), ok({ status: 1, message: "secret" })],
      operation: "PCS cookies",
      status: 200,
      reason: "invalid-response",
    },
  ];
  for (const [index, test] of failures.entries()) {
    it.effect(`fails without retry or raw response disclosure: ${index}`, () =>
      Effect.gen(function* () {
        const request = fixture(test.responses);
        const result = yield* Effect.result(
          requestProtectedAccess(request, "reminders"),
        );
        expect(Result.isFailure(result)).toBe(true);
        if (Result.isFailure(result)) {
          expect(result.failure).toBeInstanceOf(ICloudPcsError);
          expect(result.failure).toMatchObject({
            operation: test.operation,
            status: test.status,
            reason: test.reason,
          });
          expect(JSON.stringify(result.failure)).not.toContain("secret");
        }
        expect(request).toHaveBeenCalledTimes(test.responses.length);
      }),
    );
  }

  it.effect("preserves request failures without retry", () =>
    Effect.gen(function* () {
      const error = { kind: "transport" };
      const request = vi.fn(() => Effect.fail(error));
      const result = yield* Effect.result(requestProtectedAccess(request, "reminders"));
      expect(Result.isFailure(result) && result.failure).toBe(error);
      expect(request).toHaveBeenCalledTimes(1);
    }),
  );

  for (const app of ["", "Reminders", "a/b", "a b", "a".repeat(65)]) {
    it.effect(`rejects invalid service names: ${JSON.stringify(app)}`, () =>
      Effect.gen(function* () {
        const request = fixture([]);
        const result = yield* Effect.result(requestProtectedAccess(request, app));
        expect(Result.isFailure(result) && result.failure).toMatchObject({
          reason: "invalid-app-name",
        });
        expect(request).not.toHaveBeenCalled();
      }),
    );
  }
});
