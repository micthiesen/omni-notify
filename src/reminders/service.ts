import { createHash, randomUUID } from "node:crypto";
import { Clock, Data, Effect, Semaphore } from "effect";
import { AppleRemindersClient, AppleRemindersError } from "./apple.js";
import {
  RemindersCloudKitClient,
  RemindersError,
  type ReminderCreateInput,
  type ReminderPatch,
} from "./cloudkit.js";
import { remindersConfigured, type RemindersConfiguration } from "./config.js";
import type {
  RemindersControl,
  RemindersDiagnostic,
  RemindersPublicStatus,
} from "./routes.js";
import {
  createRemindersStore,
  emptyRemindersState,
  type RemindersStore,
  type RemindersStoredState,
} from "./store.js";

export class RemindersServiceError extends Data.TaggedError("RemindersServiceError")<{
  readonly code:
    | "disabled"
    | "authentication-needed"
    | "awaiting-device-approval"
    | "stale-challenge"
    | "rate-limited"
    | "uncertain-write"
    | "idempotency-conflict"
    | "conflict"
    | "not-found"
    | "storage";
}> {
  override get message() {
    return `Reminders: ${this.code}`;
  }
}
const failure = (code: RemindersServiceError["code"]) =>
  new RemindersServiceError({ code });
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, canonical(item)]),
    );
  return value;
}
const fingerprint = (value: unknown) =>
  createHash("sha256")
    .update(JSON.stringify(canonical(value)))
    .digest("hex");

interface Dependencies {
  logFailure?: (diagnostic: RemindersDiagnostic) => Effect.Effect<void>;
  store?: RemindersStore;
  apple?: Pick<
    AppleRemindersClient,
    "begin" | "verify" | "submit2fa" | "requestPcsAccess" | "ckPost"
  >;
  notify: Effect.Effect<void, unknown>;
}

function appleDiagnostic(error: AppleRemindersError): RemindersDiagnostic {
  let stage: RemindersDiagnostic["stage"];
  switch (error.operation) {
    case "SRP init":
      stage = "sign-in-init";
      break;
    case "SRP proof":
      stage = "sign-in-proof";
      break;
    case "SRP complete":
      stage = "sign-in-complete";
      break;
    case "account login":
    case "validate session":
      stage = "account-session";
      break;
    case "MFA options":
      stage = "second-factor-options";
      break;
    case "MFA push":
      stage = "device-notification";
      break;
    case "MFA verify":
      stage = "code-verification";
      break;
    case "SMS request":
    case "trust browser":
      stage = "second-factor";
      break;
    case "load session":
    case "save session":
      stage = "private-storage";
      break;
    case "PCS state":
    case "PCS consent":
    case "PCS cookies":
      stage = "protected-data-access";
      break;
    default:
      stage = "apple-request";
  }
  const httpStatus =
    Number.isInteger(error.status) && error.status! >= 100 && error.status! <= 599
      ? error.status
      : undefined;
  return {
    stage,
    category:
      stage === "private-storage"
        ? "storage"
        : httpStatus !== undefined
          ? "apple-response"
          : error.kind === "unsupported-protocol"
            ? "protocol"
            : error.kind === "transient-outage"
              ? "transport"
              : "authentication",
    ...(httpStatus !== undefined ? { httpStatus } : {}),
  };
}

/** One serialized account owner. Interrupted mutations remain reserved, never replayed. */
export class RemindersService implements RemindersControl {
  private readonly lock = Semaphore.makeUnsafe(1);
  private readonly enabled: boolean;
  private readonly store?: RemindersStore;
  private readonly apple?: Dependencies["apple"];
  private readonly cloud?: RemindersCloudKitClient;
  private stored: RemindersStoredState = emptyRemindersState();
  private loaded = false;
  private current: RemindersPublicStatus;
  private challenge?: { id: string; expires: number; attempts: number };
  private nextAuthAt = 0;

  constructor(
    config: RemindersConfiguration,
    private readonly deps: Dependencies,
  ) {
    this.enabled = remindersConfigured(config);
    this.current = {
      enabled: this.enabled,
      phase: this.enabled ? "authentication-needed" : "disabled",
      reason: this.enabled ? "credentials" : "configuration",
    };
    if (!this.enabled) return;
    this.store =
      deps.store ??
      createRemindersStore(config.directory, config.storageKey!, config.account!);
    this.apple =
      deps.apple ??
      new AppleRemindersClient({
        account: config.account!,
        password: config.password!,
        loadSession: () => this.load().pipe(Effect.map(() => this.stored.session)),
        saveSession: (session) =>
          Effect.gen({ self: this }, function* () {
            this.stored = { ...this.stored, session };
            yield* this.save();
          }),
      });
    this.cloud = new RemindersCloudKitClient((path, body) =>
      this.apple!.ckPost(
        path as
          | "/changes/zone"
          | "/records/query"
          | "/records/lookup"
          | "/records/modify",
        body,
      ).pipe(
        Effect.tapError((error) => this.recordFailure(error)),
        Effect.mapError(
          () => new RemindersError({ operation: "request", code: "transport" }),
        ),
      ),
    );
  }

  private load() {
    return Effect.gen({ self: this }, function* () {
      if (!this.enabled || !this.store) return yield* Effect.fail(failure("disabled"));
      if (!this.loaded) {
        this.stored = yield* this.store
          .read()
          .pipe(Effect.mapError(() => failure("storage")));
        this.loaded = true;
      }
    });
  }
  private save() {
    return this.store!.write(this.stored).pipe(
      Effect.mapError(() => failure("storage")),
      Effect.uninterruptible,
    );
  }

  private notifyOnce() {
    return Effect.gen({ self: this }, function* () {
      if (this.stored.notified) return;
      this.stored = { ...this.stored, notified: true };
      yield* this.save(); // Reserve before delivery; uncertain delivery is not repeated.
      yield* this.deps.notify.pipe(Effect.catch(() => Effect.void));
    });
  }

  private recordFailure(error: unknown) {
    return Effect.gen({ self: this }, function* () {
      if (error instanceof AppleRemindersError) {
        const diagnostic = appleDiagnostic(error);
        this.current = { enabled: true, phase: error.kind, diagnostic };
        yield* this.deps.logFailure?.(diagnostic) ?? Effect.void;
        if (
          error.kind === "authentication-needed" ||
          error.kind === "awaiting-device-approval" ||
          error.kind === "terms-required"
        )
          yield* this.notifyOnce();
      } else if (
        error instanceof RemindersError &&
        error.code === "awaiting-device-approval"
      ) {
        this.challenge = undefined;
        this.current = {
          enabled: true,
          phase: "awaiting-device-approval",
          reason: "pcs",
        };
        yield* this.notifyOnce();
      } else if (error instanceof RemindersError && error.code === "protocol") {
        this.current = {
          enabled: true,
          phase: "unsupported-protocol",
          reason: "protocol",
          diagnostic: { stage: "apple-request", category: "protocol" },
        };
        yield* this.deps.logFailure?.(this.current.diagnostic!) ?? Effect.void;
        yield* this.notifyOnce();
      } else if (error instanceof RemindersServiceError && error.code === "storage") {
        const diagnostic: RemindersDiagnostic = {
          stage: "private-storage",
          category: "storage",
        };
        this.current = { enabled: this.enabled, phase: "transient-outage", diagnostic };
        yield* this.deps.logFailure?.(diagnostic) ?? Effect.void;
      } else if (
        error instanceof RemindersServiceError &&
        error.code === "rate-limited"
      ) {
        this.current = { ...this.current, phase: "rate-limited" };
      }
    });
  }

  status() {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const active = this.challenge && this.challenge.expires > now;
      return {
        ...this.current,
        ...(active
          ? {
              challengeId: this.challenge!.id,
              challengeExpiresAt: this.challenge!.expires,
            }
          : {}),
      };
    });
  }

  private authenticated() {
    return Effect.gen({ self: this }, function* () {
      yield* this.cloud!.verifyReadAccess(); // Decode protected content before claiming access.
      this.challenge = undefined;
      this.current = { enabled: true, phase: "authenticated" };
      this.stored = { ...this.stored, notified: false };
      yield* this.save();
      return yield* this.status();
    });
  }

  private checkAccess(explicit: boolean) {
    return Effect.gen({ self: this }, function* () {
      yield* this.load();
      if (!(yield* this.apple!.verify())) {
        this.current = {
          enabled: true,
          phase: "authentication-needed",
          reason: "session-expired",
        };
        yield* this.notifyOnce();
        return yield* this.status();
      }
      if (explicit) {
        const pcs = yield* this.apple!.requestPcsAccess();
        if (pcs === "consent-required") {
          this.current = {
            enabled: true,
            phase: "awaiting-device-approval",
            reason: "pcs",
          };
          yield* this.notifyOnce();
          return yield* this.status();
        }
      }
      return yield* this.authenticated();
    });
  }

  verifyAccess() {
    return this.lock.withPermits(1)(
      this.checkAccess(true).pipe(
        Effect.tapError((error) => this.recordFailure(error)),
        Effect.catch(() => this.status()),
      ),
    );
  }

  /** Scheduled validation never signs in, submits codes, or requests device consent. */
  healthCheck() {
    if (!this.enabled) return Effect.void;
    return this.lock.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const now = yield* Clock.currentTimeMillis;
        if (this.challenge && this.challenge.expires <= now) this.challenge = undefined;
        if (
          this.challenge ||
          this.current.phase === "awaiting-device-approval" ||
          this.current.phase === "terms-required"
        )
          return;
        yield* this.checkAccess(false);
      }).pipe(
        Effect.tapError((error) => this.recordFailure(error)),
        Effect.catch(() => Effect.void),
      ),
    );
  }

  startAuthentication() {
    return this.lock.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        yield* this.load();
        const now = yield* Clock.currentTimeMillis;
        if (this.challenge && now < this.challenge.expires) return yield* this.status();
        if (this.current.phase === "authenticated") return yield* this.status();
        if (now < this.nextAuthAt) return yield* Effect.fail(failure("rate-limited"));
        this.nextAuthAt = now + 10 * 60_000;
        this.challenge = undefined;
        const result = yield* this.apple!.begin();
        if (result === "ready") return yield* this.checkAccess(true);
        this.challenge = { id: randomUUID(), expires: now + 10 * 60_000, attempts: 0 };
        this.current = { enabled: true, phase: "authentication-needed", reason: "mfa" };
        yield* this.notifyOnce();
        return yield* this.status();
      }).pipe(Effect.tapError((error) => this.recordFailure(error))),
    );
  }

  submitCode(input: { challengeId: string; code: string }) {
    return this.lock.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        yield* this.load();
        const now = yield* Clock.currentTimeMillis;
        const challenge = this.challenge;
        if (
          !challenge ||
          challenge.id !== input.challengeId ||
          challenge.expires <= now ||
          !/^\d{6}$/.test(input.code)
        )
          return yield* Effect.fail(failure("stale-challenge"));
        if (challenge.attempts >= 5) return yield* Effect.fail(failure("rate-limited"));
        challenge.attempts++;
        yield* this.apple!.submit2fa(input.code);
        this.challenge = undefined; // A verified code is consumed even if CloudKit is unavailable.
        return yield* this.checkAccess(true).pipe(
          Effect.catchIf(
            (error) =>
              error instanceof RemindersError &&
              error.code === "awaiting-device-approval",
            (error) => this.recordFailure(error).pipe(Effect.andThen(this.status())),
          ),
        );
      }).pipe(Effect.tapError((error) => this.recordFailure(error))),
    );
  }

  private ready() {
    return Effect.gen({ self: this }, function* () {
      yield* this.load();
      if (this.current.phase === "awaiting-device-approval")
        return yield* Effect.fail(failure("awaiting-device-approval"));
      if (this.current.phase !== "authenticated")
        return yield* Effect.fail(failure("authentication-needed"));
    });
  }

  snapshot() {
    return this.lock.withPermits(1)(
      this.ready().pipe(
        Effect.andThen(() => this.cloud!.readSnapshot()),
        Effect.tapError((error) => this.recordFailure(error)),
      ),
    );
  }
  get(id: string) {
    return this.lock.withPermits(1)(
      this.ready().pipe(
        Effect.andThen(() => this.cloud!.getReminder(id)),
        Effect.tapError((error) => this.recordFailure(error)),
      ),
    );
  }

  private mutation<A>(
    key: string,
    input: unknown,
    recordId: string,
    run: Effect.Effect<A, RemindersError | RemindersServiceError>,
  ) {
    return this.lock.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        yield* this.ready();
        if (!/^[A-Za-z0-9_-]{16,128}$/.test(key))
          return yield* Effect.fail(failure("idempotency-conflict"));
        const keyHash = fingerprint(key);
        const digest = fingerprint(input);
        const existing = this.stored.operations[keyHash];
        if (existing) {
          if (existing.fingerprint !== digest)
            return yield* Effect.fail(failure("idempotency-conflict"));
          if (existing.state === "reserved")
            return yield* Effect.fail(failure("uncertain-write"));
          return existing.result as A;
        }
        if (Object.keys(this.stored.operations).length >= 10_000)
          return yield* Effect.fail(failure("storage"));
        this.stored = {
          ...this.stored,
          operations: {
            ...this.stored.operations,
            [keyHash]: { fingerprint: digest, recordId, state: "reserved" },
          },
        };
        yield* this.save();
        const result = yield* run;
        this.stored = {
          ...this.stored,
          operations: {
            ...this.stored.operations,
            [keyHash]: { fingerprint: digest, recordId, state: "confirmed", result },
          },
        };
        yield* this.save();
        return result;
      }),
    );
  }

  create(key: string, input: Omit<ReminderCreateInput, "id">) {
    const id = `Reminder/${fingerprint(key).slice(0, 32).toUpperCase()}`;
    return this.mutation(
      key,
      { operation: "create", ...input },
      id,
      Effect.suspend(() => this.cloud!.createReminder({ ...input, id })),
    );
  }
  update(key: string, id: string, changeTag: string, patch: ReminderPatch) {
    return this.mutation(
      key,
      { operation: "update", id, changeTag, patch },
      id,
      Effect.gen({ self: this }, function* () {
        const current = yield* this.cloud!.getReminder(id);
        if (!current) return yield* Effect.fail(failure("not-found"));
        if (current.recordChangeTag !== changeTag)
          return yield* Effect.fail(failure("conflict"));
        return yield* this.cloud!.updateReminder(current, patch);
      }),
    );
  }
  delete(key: string, id: string, changeTag: string) {
    return this.mutation(
      key,
      { operation: "delete", id, changeTag },
      id,
      Effect.gen({ self: this }, function* () {
        const current = yield* this.cloud!.getReminder(id);
        if (!current) return yield* Effect.fail(failure("not-found"));
        if (current.recordChangeTag !== changeTag)
          return yield* Effect.fail(failure("conflict"));
        yield* this.cloud!.deleteReminder(current);
        return { id, deleted: true as const, verified: true as const };
      }),
    );
  }
}
