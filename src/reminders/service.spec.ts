import { describe, expect, it } from "@effect/vitest";
import { Deferred, Effect, Fiber, Result } from "effect";
import { TestClock } from "effect/testing";
import { vi } from "vitest";
import { AppleRemindersError, type AppleRemindersClient } from "./apple.js";
import { encodeCrdtDocument } from "./codec.js";
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
  background?: (effect: Effect.Effect<void>) => Effect.Effect<void>;
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
    background: options?.background,
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

const encryptedSnapshot = (encrypted: () => boolean) => (path: string, body: unknown) =>
  Effect.succeed(
    path === "/changes/zone" &&
      (body as { zones: { reverse?: boolean }[] }).zones[0].reverse
      ? {
          zones: [
            {
              records: encrypted()
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
        }
      : path === "/changes/zone"
        ? {
            zones: [
              {
                records: [
                  {
                    recordName: "List/test",
                    recordType: "List",
                    fields: { Name: { type: "STRING", value: "Test" } },
                  },
                ],
              },
            ],
          }
        : {
            records: encrypted()
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
  );

describe("Reminders service", () => {
  it.effect("reports a missing completed cursor instead of synchronizing forever", () =>
    Effect.gen(function* () {
      const jobs: Effect.Effect<void>[] = [];
      const x = fixture({
        verify: () => Effect.succeed(true),
        background: (effect) =>
          Effect.sync(() => {
            jobs.push(effect);
          }),
      });
      yield* x.service.verifyAccess();
      yield* jobs.shift()!;
      expect((yield* x.service.status()).phase).toBe("unsupported-protocol");
      expect(Result.isFailure(yield* x.service.snapshot().pipe(Effect.result))).toBe(
        true,
      );
      expect(jobs).toHaveLength(0);
    }),
  );

  it.effect(
    "rebuilds an unusable cursor on the next access check without signing in",
    () =>
      Effect.gen(function* () {
        const jobs: Effect.Effect<void>[] = [];
        let failRefresh = false;
        let fullScans = 0;
        const x = fixture({
          verify: () => Effect.succeed(true),
          background: (effect) =>
            Effect.sync(() => {
              jobs.push(effect);
            }),
          ckPost: (_path, body) => {
            const zone = (
              body as { zones: { reverse?: boolean; syncToken?: string }[] }
            ).zones[0];
            if (zone.reverse) return Effect.succeed({ zones: [{ records: [] }] });
            if (!zone.syncToken) fullScans++;
            return Effect.succeed({
              zones: [
                failRefresh && zone.syncToken
                  ? { error: { serverErrorCode: "UNKNOWN_CURSOR_FAILURE" } }
                  : { records: [], syncToken: "complete" },
              ],
            });
          },
        });
        yield* x.service.verifyAccess();
        yield* jobs.shift()!;
        failRefresh = true;
        expect(Result.isFailure(yield* x.service.snapshot().pipe(Effect.result))).toBe(
          true,
        );
        expect((yield* x.service.status()).phase).toBe("unsupported-protocol");
        failRefresh = false;
        yield* x.service.verifyAccess();
        expect(jobs).toHaveLength(1);
        yield* jobs.shift()!;
        expect(yield* x.service.snapshot()).toEqual({ lists: [], reminders: [] });
        expect(fullScans).toBe(2);
        expect(x.begin).not.toHaveBeenCalled();
      }),
  );

  it.effect(
    "rechecks initialization for reads queued before authentication completes",
    () =>
      Effect.gen(function* () {
        const jobs: Effect.Effect<void>[] = [];
        const x = fixture({
          verify: () => Effect.sleep("1 second").pipe(Effect.as(true)),
          background: (effect) =>
            Effect.sync(() => {
              jobs.push(effect);
            }),
        });
        const auth = yield* Effect.forkChild(x.service.verifyAccess());
        yield* TestClock.adjust("500 millis");
        const read = yield* Effect.forkChild(x.service.snapshot().pipe(Effect.result));
        yield* TestClock.adjust("500 millis");
        expect((yield* Fiber.join(auth)).phase).toBe("authenticated");
        const result = yield* Fiber.join(read);
        expect(Result.isFailure(result) && result.failure).toMatchObject({
          code: "synchronizing",
        });
        expect(jobs).toHaveLength(1);
        expect(x.ckPost).toHaveBeenCalledTimes(1);
      }),
  );

  it.effect(
    "registers the background task even if the initiating request is interrupted",
    () =>
      Effect.scoped(
        Effect.gen(function* () {
          const scope = yield* Effect.scope;
          const entered = yield* Deferred.make<void>();
          const release = yield* Deferred.make<void>();
          const completed = yield* Deferred.make<void>();
          const x = fixture({
            verify: () => Effect.succeed(true),
            background: (effect) =>
              Deferred.succeed(entered, undefined).pipe(
                Effect.andThen(Deferred.await(release)),
                Effect.andThen(
                  Effect.forkIn(
                    effect.pipe(
                      Effect.ensuring(Deferred.succeed(completed, undefined)),
                    ),
                    scope,
                    { uninterruptible: false },
                  ),
                ),
                Effect.asVoid,
              ),
            ckPost: () =>
              Effect.succeed({ zones: [{ records: [], syncToken: "complete" }] }),
          });
          const request = yield* Effect.forkChild(x.service.verifyAccess());
          yield* Deferred.await(entered);
          const interrupted = yield* Effect.forkChild(Fiber.interrupt(request));
          yield* Effect.yieldNow;
          yield* Deferred.succeed(release, undefined);
          yield* Fiber.join(interrupted);
          yield* Deferred.await(completed);
          expect(yield* x.service.snapshot()).toEqual({ lists: [], reminders: [] });
        }),
      ),
  );

  it.effect(
    "keeps initial synchronization in the application scope and fails fast while loading",
    () =>
      Effect.scoped(
        Effect.gen(function* () {
          const scope = yield* Effect.scope;
          const background = vi.fn((effect: Effect.Effect<void>) =>
            Effect.forkIn(effect, scope).pipe(Effect.asVoid),
          );
          const x = fixture({
            verify: () => Effect.succeed(true),
            background,
            ckPost: (_path, body) => {
              const zone = (
                body as { zones: { reverse?: boolean; syncToken?: string }[] }
              ).zones[0];
              if (zone.reverse) return Effect.succeed({ zones: [{ records: [] }] });
              const response = Effect.succeed({
                zones: [{ records: [], syncToken: "complete" }],
              });
              return zone.syncToken
                ? response
                : Effect.sleep("2 minutes").pipe(Effect.andThen(response));
            },
          });
          expect((yield* Effect.scoped(x.service.verifyAccess())).phase).toBe(
            "authenticated",
          );
          const loading = yield* x.service.snapshot().pipe(Effect.result);
          expect(Result.isFailure(loading) && loading.failure).toMatchObject({
            code: "synchronizing",
          });
          const lookup = yield* x.service.get("Reminder/test").pipe(Effect.result);
          expect(Result.isFailure(lookup) && lookup.failure).toMatchObject({
            code: "synchronizing",
          });
          expect((yield* x.service.verifyAccess()).phase).toBe("authenticated");
          expect(background).toHaveBeenCalledTimes(1);
          yield* TestClock.adjust("2 minutes");
          expect(yield* x.service.snapshot()).toEqual({ lists: [], reminders: [] });
          expect(background).toHaveBeenCalledTimes(1);
        }),
      ),
  );

  it.effect("reports accepted code separately from protected-data access", () =>
    Effect.gen(function* () {
      const x = fixture({
        verify: () => Effect.succeed(true),
        ckPost: encryptedSnapshot(() => true),
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
          ckPost: encryptedSnapshot(() => encrypted),
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

  it.effect("never replays uncertain recurring completion after service restart", () =>
    Effect.gen(function* () {
      const reminderId = "Reminder/12345678";
      const ruleId = "RecurrenceRule/ABCDEFGH";
      const reminder = {
        recordName: reminderId,
        recordType: "Reminder",
        recordChangeTag: "reminder-tag",
        fields: {
          TitleDocument: {
            type: "STRING",
            value: encodeCrdtDocument("Disposable fixture"),
          },
          List: { type: "REFERENCE", value: { recordName: "List/fixture" } },
          DueDate: { type: "TIMESTAMP", value: 1793466000000 },
          RecurrenceRuleIDs: { type: "STRING_LIST", value: ["ABCDEFGH"] },
        },
      };
      const rule = {
        recordName: ruleId,
        recordType: "RecurrenceRule",
        recordChangeTag: "rule-tag",
        fields: {
          Reminder: { type: "REFERENCE", value: { recordName: reminderId } },
          Frequency: { type: "INT64", value: 0 },
          Interval: { type: "INT64", value: 1 },
        },
      };
      const x = fixture({
        verify: () => Effect.succeed(true),
        ckPost: (path, body) => {
          if (path === "/records/lookup")
            return Effect.succeed({
              records: [
                (body as { records: { recordName: string }[] }).records[0]
                  .recordName === reminderId
                  ? reminder
                  : rule,
              ],
            });
          if (path === "/records/query")
            return (body as { query: { recordType: string } }).query.recordType ===
              "CompleteRecurringReminder"
              ? Effect.fail(authError("transient-outage"))
              : Effect.succeed({ records: [reminder, rule] });
          return Effect.succeed({ zones: [{ records: [] }] });
        },
      });
      yield* x.service.verifyAccess();
      const run = (service: RemindersService) =>
        service.completeRecurring(
          "completion-key-123456",
          reminderId,
          "reminder-tag",
          ruleId,
          "rule-tag",
          "America/Vancouver",
        );
      const first = yield* run(x.service).pipe(Effect.result);
      expect(Result.isFailure(first) && first.failure.code).toBe("uncertain");
      expect(Object.values(x.getState().operations)[0].state).toBe("reserved");
      const restarted = new RemindersService(config, {
        store: x.store,
        apple: x.apple,
        notify: Effect.void,
      });
      yield* restarted.verifyAccess();
      const second = yield* run(restarted).pipe(Effect.result);
      expect(Result.isFailure(second) && second.failure.code).toBe("uncertain-write");
      expect(
        x.ckPost.mock.calls.filter(
          ([path, body]) =>
            path === "/records/query" &&
            (body as { query: { recordType: string } }).query.recordType ===
              "CompleteRecurringReminder",
        ),
      ).toHaveLength(1);
    }),
  );

  it.effect(
    "reserves recurrence writes across restart and never replays a lost atomic response",
    () =>
      Effect.gen(function* () {
        const x = fixture({
          verify: () => Effect.succeed(true),
          ckPost: (path, body) => {
            if (path === "/records/lookup") {
              const id = (body as { records: { recordName: string }[] }).records[0]
                .recordName;
              return Effect.succeed({
                records: [
                  id.startsWith("RecurrenceRule/")
                    ? { recordName: id, serverErrorCode: "NOT_FOUND" }
                    : {
                        recordName: id,
                        recordType: "Reminder",
                        recordChangeTag: "current",
                        fields: {
                          List: {
                            type: "REFERENCE",
                            value: { recordName: "List/test" },
                          },
                          RecurrenceRuleIDs: { type: "STRING_LIST", value: [] },
                        },
                      },
                ],
              });
            }
            if (path === "/records/query") return Effect.succeed({ records: [] });
            if (path === "/records/modify")
              return Effect.fail(authError("transient-outage"));
            return Effect.succeed({ zones: [{ records: [] }] });
          },
        });
        yield* x.service.verifyAccess();
        const run = (service: RemindersService) =>
          service.createRecurrence(
            "recurrence-ledger-key",
            "Reminder/12345678",
            "current",
            { frequency: "daily", interval: 1 },
          );
        expect(Result.isFailure(yield* run(x.service).pipe(Effect.result))).toBe(true);
        expect(Object.values(x.getState().operations)[0]).toMatchObject({
          state: "reserved",
          recordId: expect.stringMatching(/^RecurrenceRule\//),
        });
        const restarted = new RemindersService(config, {
          store: x.store,
          apple: x.apple,
          notify: Effect.void,
        });
        yield* restarted.verifyAccess();
        const replay = yield* run(restarted).pipe(Effect.result);
        expect(Result.isFailure(replay) && replay.failure.code).toBe("uncertain-write");
        expect(
          x.ckPost.mock.calls.filter(([path]) => path === "/records/modify"),
        ).toHaveLength(1);
      }),
  );

  it.effect("returns confirmed list rename receipts without repeating writes", () =>
    Effect.gen(function* () {
      let title = "Original";
      let tag = "old";
      const x = fixture({
        verify: () => Effect.succeed(true),
        ckPost: (path) => {
          if (path === "/records/lookup")
            return Effect.succeed({
              records: [
                {
                  recordName: "List/test",
                  recordType: "List",
                  recordChangeTag: tag,
                  fields: { Name: { type: "STRING", value: title } },
                },
              ],
            });
          if (path === "/records/modify") {
            title = "Renamed";
            tag = "new";
            return Effect.succeed({
              records: [{ recordName: "List/test", recordChangeTag: tag }],
            });
          }
          return Effect.succeed({ zones: [{ records: [] }] });
        },
      });
      yield* x.service.verifyAccess();
      const run = () =>
        x.service.updateList("list-rename-key-123", "List/test", "old", "Renamed");
      const first = yield* run();
      expect(yield* run()).toEqual(first);
      expect(
        x.ckPost.mock.calls.filter(([path]) => path === "/records/modify"),
      ).toHaveLength(1);
      const conflict = yield* x.service
        .updateList("list-rename-key-123", "List/test", "old", "Different")
        .pipe(Effect.result);
      expect(Result.isFailure(conflict) && conflict.failure.code).toBe(
        "idempotency-conflict",
      );
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
