import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber, Result } from "effect";
import { TestClock } from "effect/testing";
import { vi } from "vitest";
import { AppleRemindersError, type AppleRemindersClient } from "./apple.js";
import type { RemindersConfiguration } from "./config.js";
import { RemindersService } from "./service.js";
import type { RemindersDiagnostic } from "./routes.js";
import {
  emptyRemindersState,
  type RemindersStore,
  type RemindersStoredState,
} from "./store.js";

const config: RemindersConfiguration = {
  enabled: "true",
  account: "test@example.com",
  password: "never-sent",
  storageKey: "a".repeat(64),
  publicOrigin: "https://omni.example.test",
  directory: "/tmp/omni-reminders-service-test-unused",
};

function fixture(options?: {
  begin?: () => Effect.Effect<"ready" | "mfa-required", AppleRemindersError>;
  verify?: () => Effect.Effect<boolean, AppleRemindersError>;
  submit2fa?: () => Effect.Effect<"ready", AppleRemindersError>;
  ckPost?: (path: string, body: unknown) => Effect.Effect<unknown, AppleRemindersError>;
  state?: RemindersStoredState;
  logFailure?: (diagnostic: RemindersDiagnostic) => Effect.Effect<void>;
}) {
  let state = options?.state ?? emptyRemindersState();
  const read = vi.fn(() => Effect.sync(() => structuredClone(state)));
  const write = vi.fn((next: RemindersStoredState) =>
    Effect.sync(() => {
      state = structuredClone(next);
    }),
  );
  const store: RemindersStore = { read, write };
  const begin = vi.fn(
    options?.begin ?? (() => Effect.succeed("mfa-required" as const)),
  );
  const verify = vi.fn(options?.verify ?? (() => Effect.succeed(false)));
  const submit2fa = vi.fn(
    options?.submit2fa ?? (() => Effect.succeed("ready" as const)),
  );
  const requestPcsAccess = vi.fn(() => Effect.succeed("not-required" as const));
  const ckPost = vi.fn(
    (path: string, body: unknown) =>
      options?.ckPost?.(path, body) ?? Effect.succeed({ zones: [{ records: [] }] }),
  );
  const notify = vi.fn(() => Effect.void);
  const apple = {
    begin,
    verify,
    submit2fa,
    requestPcsAccess,
    ckPost,
  } as unknown as Pick<
    AppleRemindersClient,
    "begin" | "verify" | "submit2fa" | "requestPcsAccess" | "ckPost"
  >;
  const service = new RemindersService(config, {
    logFailure: options?.logFailure,
    store,
    apple,
    notify: Effect.sync(() => {
      notify();
    }),
  });
  return {
    service,
    store,
    read,
    write,
    begin,
    verify,
    submit2fa,
    requestPcsAccess,
    ckPost,
    notify,
    getState: () => state,
    apple,
  };
}

const authError = (kind: AppleRemindersError["kind"]) =>
  new AppleRemindersError({ operation: "fixture", reason: "opaque", kind });

describe("Reminders service", () => {
  it.effect("reports accepted code separately from protected-data access", () =>
    Effect.gen(function* () {
      const x = fixture({
        verify: () => Effect.succeed(true),
        ckPost: () =>
          Effect.succeed({
            zones: [
              {
                records: [
                  {
                    recordName: "fixture",
                    recordType: "Reminder",
                    recordChangeTag: "tag",
                    fields: {
                      TitleDocument: { type: "ENCRYPTED_BYTES", value: "opaque" },
                    },
                  },
                ],
              },
            ],
          }),
      });
      const challenge = yield* x.service.startAuthentication();
      const status = yield* x.service.submitCode({
        challengeId: challenge.challengeId!,
        code: "123456",
      });
      expect(status).toMatchObject({
        phase: "awaiting-device-approval",
        reason: "pcs",
      });
      expect(status.challengeId).toBeUndefined();
      expect(x.submit2fa).toHaveBeenCalledTimes(1);
    }),
  );

  it.effect(
    "keeps encrypted data awaiting approval without repeating sign-in or consent",
    () =>
      Effect.gen(function* () {
        let encrypted = true;
        const x = fixture({
          verify: () => Effect.succeed(true),
          ckPost: () =>
            Effect.succeed({
              zones: [
                {
                  records: encrypted
                    ? [
                        {
                          recordName: "fixture",
                          recordType: "Reminder",
                          recordChangeTag: "tag",
                          fields: {
                            TitleDocument: { type: "ENCRYPTED_BYTES", value: "opaque" },
                          },
                        },
                      ]
                    : [],
                },
              ],
            }),
        });
        yield* x.service.healthCheck();
        expect(yield* x.service.status()).toMatchObject({
          phase: "awaiting-device-approval",
          reason: "pcs",
        });
        yield* x.service.healthCheck();
        expect(x.ckPost).toHaveBeenCalledTimes(1);
        expect(x.requestPcsAccess).not.toHaveBeenCalled();
        expect(x.begin).not.toHaveBeenCalled();
        const blocked = yield* x.service.snapshot().pipe(Effect.result);
        expect(Result.isFailure(blocked) && blocked.failure.message).toContain(
          "awaiting-device-approval",
        );
        encrypted = false;
        expect(yield* x.service.verifyAccess()).toMatchObject({
          phase: "authenticated",
        });
        expect(x.requestPcsAccess).toHaveBeenCalledTimes(1);
        expect(x.ckPost).toHaveBeenCalledTimes(2);
      }),
  );

  it.each([
    ["MFA options", "second-factor-options"],
    ["MFA push", "device-notification"],
    ["MFA verify", "code-verification"],
  ])("distinguishes %s failures", async (operation, stage) => {
    const x = fixture({
      begin: () =>
        Effect.fail(
          new AppleRemindersError({
            operation,
            reason: "private response",
            status: 405,
            kind: "unsupported-protocol",
          }),
        ),
    });
    await Effect.runPromise(x.service.startAuthentication().pipe(Effect.result));
    expect((await Effect.runPromise(x.service.status())).diagnostic).toEqual({
      stage,
      category: "apple-response",
      httpStatus: 405,
    });
  });

  it.effect("reports and logs only bounded sign-in diagnostics", () =>
    Effect.gen(function* () {
      const diagnostics: RemindersDiagnostic[] = [];
      const x = fixture({
        begin: () =>
          Effect.fail(
            new AppleRemindersError({
              operation: "SRP init",
              reason: "secret upstream cookies",
              status: 503,
              kind: "transient-outage",
            }),
          ),
        logFailure: (diagnostic) =>
          Effect.sync(() => {
            diagnostics.push(diagnostic);
          }),
      });
      yield* x.service.startAuthentication().pipe(Effect.result);
      const expected = {
        stage: "sign-in-init",
        category: "apple-response",
        httpStatus: 503,
      };
      expect((yield* x.service.status()).diagnostic).toEqual(expected);
      expect(diagnostics).toEqual([expected]);
      yield* x.service.startAuthentication().pipe(Effect.result);
      expect((yield* x.service.status()).phase).toBe("rate-limited");
      expect(x.begin).toHaveBeenCalledTimes(1);
    }),
  );

  it.effect("does not expose unknown Apple operation or invalid HTTP status", () =>
    Effect.gen(function* () {
      const x = fixture({
        begin: () =>
          Effect.fail(
            new AppleRemindersError({
              operation: "secret account URL",
              reason: "secret",
              status: 900,
              kind: "unsupported-protocol",
            }),
          ),
      });
      yield* x.service.startAuthentication().pipe(Effect.result);
      expect((yield* x.service.status()).diagnostic).toEqual({
        stage: "apple-request",
        category: "protocol",
      });
    }),
  );

  it.effect(
    "keeps incomplete configuration disabled without storage or network work",
    () =>
      Effect.gen(function* () {
        const x = fixture();
        const disabled = new RemindersService(
          { ...config, storageKey: undefined },
          {
            store: x.store,
            apple: x.apple,
            notify: Effect.void,
          },
        );
        expect((yield* disabled.status()).phase).toBe("disabled");
        yield* disabled.healthCheck();
        const start = yield* disabled.startAuthentication().pipe(Effect.result);
        expect(Result.isFailure(start)).toBe(true);
        expect(x.read).not.toHaveBeenCalled();
        expect(x.begin).not.toHaveBeenCalled();
        expect(x.verify).not.toHaveBeenCalled();
      }),
  );

  it.effect("reuses one challenge and rejects stale codes after its deadline", () =>
    Effect.gen(function* () {
      const x = fixture();
      const first = yield* x.service.startAuthentication();
      const second = yield* x.service.startAuthentication();
      expect(first.challengeId).toBeTruthy();
      expect(second.challengeId).toBe(first.challengeId);
      expect(x.begin).toHaveBeenCalledTimes(1);
      yield* TestClock.adjust("11 minutes");
      const stale = yield* x.service
        .submitCode({ challengeId: first.challengeId!, code: "123456" })
        .pipe(Effect.result);
      expect(Result.isFailure(stale)).toBe(true);
      expect(x.submit2fa).not.toHaveBeenCalled();
    }),
  );

  it.effect("serializes concurrent authentication starts", () =>
    Effect.gen(function* () {
      const x = fixture();
      const [first, second] = yield* Effect.all(
        [x.service.startAuthentication(), x.service.startAuthentication()],
        { concurrency: "unbounded" },
      );
      expect(first.challengeId).toBe(second.challengeId);
      expect(x.begin).toHaveBeenCalledTimes(1);
    }),
  );

  it.effect("caps failed code attempts without issuing another Apple request", () =>
    Effect.gen(function* () {
      const x = fixture({
        submit2fa: () => Effect.fail(authError("authentication-needed")),
      });
      const status = yield* x.service.startAuthentication();
      for (let attempt = 0; attempt < 6; attempt++) {
        const result = yield* x.service
          .submitCode({ challengeId: status.challengeId!, code: "123456" })
          .pipe(Effect.result);
        expect(Result.isFailure(result)).toBe(true);
      }
      expect(x.submit2fa).toHaveBeenCalledTimes(5);
    }),
  );

  it.effect("invalidates an in-memory challenge across service restart", () =>
    Effect.gen(function* () {
      const x = fixture();
      const challenge = yield* x.service.startAuthentication();
      const restarted = new RemindersService(config, {
        store: x.store,
        apple: x.apple,
        notify: Effect.void,
      });
      const result = yield* restarted
        .submitCode({ challengeId: challenge.challengeId!, code: "123456" })
        .pipe(Effect.result);
      expect(Result.isFailure(result)).toBe(true);
      expect(x.submit2fa).not.toHaveBeenCalled();
    }),
  );

  it.effect("reserves one notification across repeated checks and restart", () =>
    Effect.gen(function* () {
      const x = fixture();
      yield* x.service.verifyAccess();
      yield* x.service.verifyAccess();
      const restarted = new RemindersService(config, {
        store: x.store,
        apple: x.apple,
        notify: Effect.sync(() => {
          x.notify();
        }),
      });
      yield* restarted.verifyAccess();
      expect(x.notify).toHaveBeenCalledTimes(1);
      expect(x.getState().notified).toBe(true);
      expect(x.write).toHaveBeenCalled();
    }),
  );

  it.effect("leaves an uncertain mutation reserved and never replays it", () =>
    Effect.gen(function* () {
      const x = fixture({
        verify: () => Effect.succeed(true),
        ckPost: (path, body) =>
          path === "/records/lookup"
            ? Effect.succeed({
                records: [
                  {
                    recordName: (body as { records: { recordName: string }[] })
                      .records[0].recordName,
                    serverErrorCode: "NOT_FOUND",
                  },
                ],
              })
            : path === "/records/modify"
              ? Effect.fail(authError("transient-outage"))
              : Effect.succeed({ zones: [{ records: [] }] }),
      });
      expect((yield* x.service.verifyAccess()).phase).toBe("authenticated");
      const key = "stable-request-key-1";
      const input = { listId: "List/test", title: "Milk" };
      const first = yield* x.service.create(key, input).pipe(Effect.result);
      expect(Result.isFailure(first)).toBe(true);
      expect(Object.values(x.getState().operations)[0]?.state).toBe("reserved");
      const restarted = new RemindersService(config, {
        store: x.store,
        apple: x.apple,
        notify: Effect.void,
      });
      yield* restarted.verifyAccess();
      const retry = yield* restarted.create(key, input).pipe(Effect.result);
      expect(Result.isFailure(retry)).toBe(true);
      const modifies = x.ckPost.mock.calls.filter(
        ([path]) => path === "/records/modify",
      );
      expect(modifies).toHaveLength(1);
    }),
  );

  it.effect(
    "keeps a mutation reservation after its network request is interrupted",
    () =>
      Effect.gen(function* () {
        let enteredModify = false;
        const x = fixture({
          verify: () => Effect.succeed(true),
          ckPost: (path, body) =>
            path === "/records/lookup"
              ? Effect.succeed({
                  records: [
                    {
                      recordName: (body as { records: { recordName: string }[] })
                        .records[0].recordName,
                      serverErrorCode: "NOT_FOUND",
                    },
                  ],
                })
              : path === "/records/modify"
                ? Effect.sync(() => {
                    enteredModify = true;
                  }).pipe(Effect.andThen(Effect.never))
                : Effect.succeed({ zones: [{ records: [] }] }),
        });
        yield* x.service.verifyAccess();
        const fiber = yield* Effect.forkChild(
          x.service.create("interrupted-key-123", {
            listId: "List/test",
            title: "Milk",
          }),
        );
        for (let i = 0; i < 20 && !enteredModify; i++) yield* Effect.yieldNow;
        expect(enteredModify).toBe(true);
        yield* Fiber.interrupt(fiber);
        expect(Object.values(x.getState().operations)[0]?.state).toBe("reserved");
      }),
  );
});
