import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
  randomUUID,
} from "node:crypto";
import { Data, Effect, Clock, Semaphore } from "effect";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { FetchedEmail, EmailHandler } from "../../email/types.js";
import { isArchiveActionMessageEffect } from "../../email/archive/persistence.js";
import type { ExecutorEventAuthorizer } from "./executorAuth.js";
import {
  EventPersistence,
  type EventDelivery,
  type EventDeliveryFailure,
  type EventDeliveryWithhold,
  type EventFolder,
  type EventRequestMethod,
  type EventSubscription,
} from "./persistence.js";
import {
  deliverWebhook,
  validateCallbackUrl,
  validateSigningSecret,
  verifyCallback,
} from "./webhook.js";

const EVENT_NAME = "email.received" as const;
const DEFAULT_TTL_MS = 24 * 60 * 60_000;
const MAX_TTL_MS = 7 * DEFAULT_TTL_MS;
const VERIFY_CACHE_MS = 60 * 60_000;
const ROTATION_MS = 5 * 60_000;
const MAX_ATTEMPTS = 8;
const MAX_DUE_PER_PASS = 10;
/** How long a subscription's deliveries wait before rechecking authorization. */
const WITHHELD_RECHECK_MS: Record<EventDeliveryWithhold, number> = {
  authorization_invalid: 15 * 60_000,
  authorization_unavailable: 60_000,
};
const STATUS_RECENT_DELIVERIES = 10;
/** Ask delegated clients to refresh this long before their token expires. */
const REFRESH_MARGIN_MS = 60_000;
const STATUS_SUBSCRIPTIONS = 20;

export class EventSubscriptionError extends Data.TaggedError("EventSubscriptionError")<{
  readonly reason:
    | "invalid_event"
    | "invalid_arguments"
    | "invalid_callback"
    | "invalid_principal"
    | "invalid_secret"
    | "challenge_failed"
    | "timeout";
}> {
  public override get message(): string {
    return `Event subscription rejected: ${this.reason}`;
  }
}

type WebhookPort = {
  verify: typeof verifyCallback;
  deliver: typeof deliverWebhook;
};

export interface SubscribeInput {
  name: string;
  arguments: { folder: EventFolder };
  delivery: { mode: "webhook"; url: string; secret: string };
  ttlMs?: number | null;
  cursor?: string | null;
}

export interface EventPrincipal {
  owner: string;
  authorization: string;
}

export interface UnsubscribeInput {
  name: string;
  arguments: { folder: EventFolder };
  delivery: { mode: "webhook"; url: string };
}

export function canonicalEventArguments(input: { folder: EventFolder }): string {
  return JSON.stringify({ folder: input.folder });
}

function digest(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

/** A short, non-secret label that distinguishes delegated clients. */
function ownerLabel(owner: string): string {
  return owner.startsWith("executor:") ? owner.slice(0, 21) : "direct";
}

function callbackHost(url: string | undefined): string | undefined {
  if (!url || !URL.canParse(url)) return undefined;
  return new URL(url).hostname.slice(0, 253) || undefined;
}

const iso = (time: number) => new Date(time).toISOString();

/** A refresh stored a validating token, so withheld events can be attempted now. */
const releaseWithheld = Effect.fn("Events.releaseWithheld")(function* (
  subscription: Pick<EventSubscription, "id" | "generation">,
  now: number,
) {
  for (const row of yield* EventPersistence.deliveries()) {
    if (
      row.subscriptionId === subscription.id &&
      row.subscriptionGeneration === subscription.generation &&
      row.status === "pending" &&
      row.withheld
    ) {
      yield* EventPersistence.upsertDelivery({ ...row, nextAttemptAt: now });
    }
  }
});

/** Owner identity and encryption keys derive from the MCP bearer token, never stored. */
export class EmailEventService {
  private readonly key: Buffer;
  private readonly keyId: string;
  private readonly owner: string;
  private readonly webhook: WebhookPort;
  private readonly semaphore = Semaphore.makeUnsafe(1);
  private readonly authorizeOwner?: ExecutorEventAuthorizer;

  constructor(
    token: string,
    authorizeOwner?: ExecutorEventAuthorizer,
    webhook: WebhookPort = { verify: verifyCallback, deliver: deliverWebhook },
  ) {
    this.key = createHmac("sha256", token)
      .update("omni-mcp-events-storage-v1")
      .digest();
    this.keyId = digest(this.key.toString("base64"));
    this.owner = digest(`omni-mcp-events-owner-v1:${token}`);
    this.webhook = webhook;
    this.authorizeOwner = authorizeOwner;
  }

  private seal(value: string): string {
    const nonce = randomBytes(12);
    const cipher = createCipheriv("aes-256-gcm", this.key, nonce);
    const ciphertext = Buffer.concat([cipher.update(value, "utf8"), cipher.final()]);
    return Buffer.concat([nonce, cipher.getAuthTag(), ciphertext]).toString("base64");
  }

  private open(value: string): string {
    const bytes = Buffer.from(value, "base64");
    const decipher = createDecipheriv("aes-256-gcm", this.key, bytes.subarray(0, 12));
    decipher.setAuthTag(bytes.subarray(12, 28));
    return Buffer.concat([
      decipher.update(bytes.subarray(28)),
      decipher.final(),
    ]).toString("utf8");
  }

  private openSafe(value: string): string | undefined {
    try {
      return this.open(value);
    } catch {
      return undefined;
    }
  }

  private subscriptionId(input: {
    owner: string;
    name: string;
    arguments: { folder: EventFolder };
    url: string;
  }): string {
    const material = JSON.stringify([
      input.owner,
      input.url,
      input.name,
      canonicalEventArguments(input.arguments),
    ]);
    return `sub_${createHmac("sha256", this.key).update(material).digest("hex").slice(0, 40)}`;
  }

  /** Resolves a delegated principal's token expiry; direct callers have none. */
  private authorizePrincipal(owner: string, principal?: EventPrincipal) {
    return Effect.gen({ self: this }, function* () {
      if (!principal) return undefined;
      const expiresAt = this.authorizeOwner
        ? yield* this.authorizeOwner(owner, principal.authorization)
        : null;
      if (expiresAt === null) {
        return yield* new EventSubscriptionError({ reason: "invalid_principal" });
      }
      return expiresAt;
    });
  }

  /** Best effort: diagnostics must never fail or delay an event request. */
  private recordRequest(
    method: EventRequestMethod,
    owner: string,
    outcome: string,
    folder?: string,
    url?: string,
  ) {
    return Effect.gen(function* () {
      yield* EventPersistence.recordRequest({
        id: randomUUID(),
        at: yield* Clock.currentTimeMillis,
        method,
        owner: ownerLabel(owner),
        ...(folder === "inbox" || folder === "archive" ? { folder } : {}),
        ...(callbackHost(url) ? { callbackHost: callbackHost(url) } : {}),
        outcome,
      });
    }).pipe(Effect.ignoreCause);
  }

  /** Records that a client read the event catalog; `owner` is a delegated owner. */
  recordDiscovery(owner?: string) {
    return this.recordRequest("events/list", owner ?? this.owner, "listed");
  }

  subscribe(input: SubscribeInput, principal?: EventPrincipal) {
    const owner = principal?.owner ?? this.owner;
    const record = (outcome: string) =>
      this.recordRequest(
        "events/subscribe",
        owner,
        outcome,
        input.arguments.folder,
        input.delivery.url,
      );
    return this.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const tokenExpiresAt = yield* this.authorizePrincipal(owner, principal);
        if (input.name !== EVENT_NAME) {
          return yield* new EventSubscriptionError({ reason: "invalid_event" });
        }
        if (
          input.arguments.folder !== "inbox" &&
          input.arguments.folder !== "archive"
        ) {
          return yield* new EventSubscriptionError({ reason: "invalid_arguments" });
        }
        let url: string;
        try {
          url = validateCallbackUrl(input.delivery.url).href;
        } catch {
          return yield* new EventSubscriptionError({ reason: "invalid_callback" });
        }
        try {
          validateSigningSecret(input.delivery.secret);
        } catch {
          return yield* new EventSubscriptionError({ reason: "invalid_secret" });
        }
        const id = this.subscriptionId({ ...input, owner, url });
        const now = yield* Clock.currentTimeMillis;
        const previous = yield* EventPersistence.subscription(id);
        const sameSecret =
          previous?.owner === owner &&
          this.openSafe(previous.encryptedSecret) === input.delivery.secret;
        const recentlyVerified =
          sameSecret && previous.verifiedAt + VERIFY_CACHE_MS > now;
        if (!recentlyVerified) {
          const verification = yield* this.webhook
            .verify({ id, url, secret: input.delivery.secret })
            .pipe(Effect.result);
          if (verification._tag === "Failure") {
            const reason = verification.failure.reason;
            return yield* new EventSubscriptionError({
              reason:
                reason === "timeout" ||
                reason === "invalid_secret" ||
                reason === "invalid_callback"
                  ? reason
                  : "challenge_failed",
            });
          }
        }
        const requested = input.ttlMs == null ? DEFAULT_TTL_MS : input.ttlMs;
        const ttl = Math.min(Math.max(1_000, requested), MAX_TTL_MS);
        // Delivery needs a token that validates when it is sent, so ask the
        // client to refresh by the time this one expires. The subscription's
        // lifetime and every authorization check are unchanged.
        const refreshBefore =
          tokenExpiresAt === undefined
            ? now + ttl
            : Math.min(
                now + ttl,
                tokenExpiresAt,
                Math.max(now, tokenExpiresAt - REFRESH_MARGIN_MS),
              );
        const secretChanged = previous?.owner === owner && !sameSecret;
        const row: EventSubscription = {
          id,
          owner,
          keyId: this.keyId,
          generation: previous?.generation ?? randomUUID(),
          name: EVENT_NAME,
          folder: input.arguments.folder,
          encryptedUrl: this.seal(url),
          encryptedSecret: this.seal(input.delivery.secret),
          encryptedPreviousSecret: secretChanged
            ? previous.encryptedSecret
            : previous?.encryptedPreviousSecret,
          previousSecretUntil: secretChanged
            ? now + ROTATION_MS
            : previous?.previousSecretUntil,
          encryptedAuthorization: principal
            ? this.seal(principal.authorization)
            : undefined,
          refreshBefore,
          expiresAt: now + ttl,
          verifiedAt: recentlyVerified ? previous.verifiedAt : now,
        };
        yield* EventPersistence.upsertSubscription(row);
        if (principal) yield* releaseWithheld(row, now).pipe(Effect.ignoreCause);
        return {
          refreshed: previous?.owner === owner,
          result: {
            id,
            refreshBefore: new Date(refreshBefore).toISOString(),
            cursor: null,
            truncated: false,
          },
        };
      }).pipe(
        Effect.tap(({ refreshed }) => record(refreshed ? "refreshed" : "accepted")),
        Effect.tapError((error) =>
          record(error instanceof EventSubscriptionError ? error.reason : "error"),
        ),
        Effect.map(({ result }) => result),
      ),
    );
  }

  unsubscribe(input: UnsubscribeInput, principal?: EventPrincipal) {
    const owner = principal?.owner ?? this.owner;
    const record = (outcome: string) =>
      this.recordRequest(
        "events/unsubscribe",
        owner,
        outcome,
        input.arguments.folder,
        input.delivery.url,
      );
    return this.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        yield* this.authorizePrincipal(owner, principal);
        if (input.name !== EVENT_NAME) return "not_found";
        if (input.arguments.folder !== "inbox" && input.arguments.folder !== "archive")
          return "not_found";
        let url: string;
        try {
          url = validateCallbackUrl(input.delivery.url).href;
        } catch {
          return "not_found";
        }
        const id = this.subscriptionId({ ...input, owner, url });
        const prior = yield* EventPersistence.subscription(id);
        if (prior?.owner !== owner) return "not_found";
        yield* EventPersistence.deleteSubscription(id);
        return "removed";
      }).pipe(
        Effect.tap(record),
        Effect.tapError((error) =>
          record(error instanceof EventSubscriptionError ? error.reason : "error"),
        ),
        Effect.as({}),
      ),
    );
  }

  /** Called before the IMAP cursor commits. Replayed polls preserve outbox IDs. */
  recordEmail(email: FetchedEmail) {
    return this.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const origin = email.origin;
        const folder = origin?.folder.toLowerCase();
        if (!origin || (folder !== "inbox" && folder !== "archive")) return;
        if (!email.messageId || !email.messageId.trim()) return;
        if (yield* isArchiveActionMessageEffect(email.messageId, origin)) return;
        const messageKey = digest(email.messageId);
        const priorReceipt = yield* EventPersistence.receipt(messageKey);
        if (priorReceipt) return;
        const now = yield* Clock.currentTimeMillis;
        const subscriptions = yield* EventPersistence.subscriptions();
        const timestamp = Number.isFinite(Date.parse(email.receivedAt))
          ? new Date(email.receivedAt).toISOString()
          : new Date(now).toISOString();
        const eventId = `evt_${digest(`${messageKey}:${folder}`).slice(0, 40)}`;
        const deliveries: EventDelivery[] = subscriptions
          .filter(
            (subscription) =>
              subscription.keyId === this.keyId &&
              subscription.folder === folder &&
              subscription.expiresAt > now,
          )
          .map((subscription) => ({
            id: `${subscription.id}:${eventId}`,
            subscriptionId: subscription.id,
            owner: subscription.owner,
            subscriptionGeneration: subscription.generation,
            eventId,
            name: EVENT_NAME,
            timestamp,
            data: {
              messageId: email.messageId!,
              folder,
              uidValidity: origin.uidValidity,
              uid: origin.uid,
            },
            attempts: 0,
            nextAttemptAt: now,
            status: "pending",
            createdAt: now,
            updatedAt: now,
          }));
        yield* EventPersistence.commitReceiptAndDeliveries(
          { messageKey, folder, receivedAt: now },
          deliveries,
        );
      }),
    );
  }

  /** Retry due rows after restart. Claim before network I/O; keep eventId stable. */
  drain() {
    return this.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const now = yield* Clock.currentTimeMillis;
        const pending = (yield* EventPersistence.deliveries()).filter(
          (row) => row.status === "pending",
        );
        const due = pending
          .filter((row) => row.nextAttemptAt <= now)
          .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt)
          .slice(0, MAX_DUE_PER_PASS);
        // Check each delegated subscription at most once per pass. A held
        // subscription defers all of its rows together so it cannot crowd out
        // other subscriptions or abort the pass.
        const authorized = new Map<string, number>();
        const withheld = new Set<string>();
        for (const row of due) {
          if (withheld.has(row.subscriptionId)) continue;
          const reason = yield* this.deliverOne(row, authorized);
          if (!reason) continue;
          withheld.add(row.subscriptionId);
          for (const held of pending) {
            if (held.subscriptionId !== row.subscriptionId) continue;
            yield* EventPersistence.upsertDelivery({
              ...held,
              withheld: reason,
              nextAttemptAt: Math.max(
                held.nextAttemptAt,
                now + WITHHELD_RECHECK_MS[reason],
              ),
              updatedAt: now,
            });
          }
        }
        return due.length;
      }),
    );
  }

  /** Returns why the row's subscription is withheld, if it is. */
  private deliverOne(row: EventDelivery, authorized: Map<string, number>) {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const fail = (failure: EventDeliveryFailure) =>
        EventPersistence.upsertDelivery({
          ...row,
          status: "failed",
          failure,
          withheld: undefined,
          updatedAt: now,
        }).pipe(Effect.as(undefined));
      if (row.attempts >= MAX_ATTEMPTS) return yield* fail("attempts_exhausted");
      const subscription = yield* EventPersistence.subscription(row.subscriptionId);
      if (
        !subscription ||
        subscription.keyId !== this.keyId ||
        subscription.owner !== row.owner ||
        subscription.generation !== row.subscriptionGeneration ||
        subscription.expiresAt <= now ||
        subscription.folder !== row.data.folder
      ) {
        return yield* fail("subscription_inactive");
      }
      if (subscription.owner.startsWith("executor:")) {
        const authorization =
          subscription.encryptedAuthorization &&
          this.openSafe(subscription.encryptedAuthorization);
        if (!authorization || !this.authorizeOwner) {
          return yield* fail("credentials_unavailable");
        }
        // Executor access tokens expire hourly. Never deliver without one that
        // validates now; hold the event until a refresh stores a valid token or
        // the subscription ends.
        let validUntil = authorized.get(subscription.id);
        if (validUntil === undefined) {
          const check = yield* this.authorizeOwner(
            subscription.owner,
            authorization,
          ).pipe(Effect.result);
          if (check._tag === "Failure") return "authorization_unavailable" as const;
          if (check.success === null) return "authorization_invalid" as const;
          validUntil = check.success;
          authorized.set(subscription.id, validUntil);
        }
        if (validUntil <= (yield* Clock.currentTimeMillis)) {
          return "authorization_invalid" as const;
        }
      }
      const url = this.openSafe(subscription.encryptedUrl);
      const secret = this.openSafe(subscription.encryptedSecret);
      if (!url || !secret) return yield* fail("credentials_unavailable");
      const claimed: EventDelivery = {
        ...row,
        withheld: undefined,
        attempts: row.attempts + 1,
        nextAttemptAt: now + Math.min(6 * 60 * 60_000, 30_000 * 2 ** row.attempts),
        updatedAt: now,
      };
      yield* EventPersistence.upsertDelivery(claimed);
      const response = yield* this.webhook
        .deliver({
          id: subscription.id,
          url,
          secret,
          previousSecret:
            subscription.previousSecretUntil &&
            subscription.previousSecretUntil > now &&
            subscription.encryptedPreviousSecret
              ? this.openSafe(subscription.encryptedPreviousSecret)
              : undefined,
          event: {
            eventId: row.eventId,
            name: row.name,
            timestamp: row.timestamp,
            data: row.data,
            cursor: null,
          },
        })
        .pipe(Effect.result);
      const status = response._tag === "Success" ? response.success.status : undefined;
      const rejected =
        status !== undefined &&
        status >= 300 &&
        status < 500 &&
        status !== 408 &&
        status !== 429;
      const delivered = status !== undefined && status >= 200 && status < 300;
      const exhausted = !delivered && !rejected && claimed.attempts >= MAX_ATTEMPTS;
      yield* EventPersistence.upsertDelivery({
        ...claimed,
        status: delivered ? "delivered" : rejected || exhausted ? "failed" : "pending",
        lastStatus: status,
        lastError: response._tag === "Failure" ? response.failure.reason : undefined,
        failure: rejected ? "rejected" : exhausted ? "attempts_exhausted" : undefined,
        updatedAt: yield* Clock.currentTimeMillis,
      });
    });
  }

  /** Bounded, secret-free view of every event boundary Omni controls. */
  status() {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const subscriptions = yield* EventPersistence.subscriptions();
      const deliveries = yield* EventPersistence.deliveries();
      const recent = [...deliveries]
        .sort((a, b) => b.updatedAt - a.updatedAt)
        .slice(0, STATUS_RECENT_DELIVERIES);
      return {
        checkedAt: iso(now),
        subscriptionTotal: subscriptions.length,
        subscriptions: subscriptions
          .sort((a, b) => b.expiresAt - a.expiresAt)
          .slice(0, STATUS_SUBSCRIPTIONS)
          .map((row) => ({
            id: row.id,
            folder: row.folder,
            owner: ownerLabel(row.owner),
            callbackHost:
              row.keyId === this.keyId
                ? (callbackHost(this.openSafe(row.encryptedUrl)) ?? null)
                : null,
            state:
              row.keyId !== this.keyId
                ? ("stale_key" as const)
                : row.expiresAt <= now
                  ? ("expired" as const)
                  : ("active" as const),
            refreshBefore: iso(row.refreshBefore ?? row.expiresAt),
            expiresAt: iso(row.expiresAt),
            verifiedAt: iso(row.verifiedAt),
          })),
        deliveries: {
          pending: deliveries.filter((row) => row.status === "pending").length,
          withheld: deliveries.filter((row) => row.status === "pending" && row.withheld)
            .length,
          delivered: deliveries.filter((row) => row.status === "delivered").length,
          failed: deliveries.filter((row) => row.status === "failed").length,
          recent: recent.map((row) => ({
            eventId: row.eventId,
            subscriptionId: row.subscriptionId,
            folder: row.data.folder,
            status: row.status,
            attempts: row.attempts,
            lastStatus: row.lastStatus ?? null,
            lastError: row.lastError ?? null,
            failure: row.failure ?? null,
            withheld: row.withheld ?? null,
            createdAt: iso(row.createdAt),
            updatedAt: iso(row.updatedAt),
          })),
        },
        requests: (yield* EventPersistence.requests()).map((row) => ({
          at: iso(row.at),
          method: row.method,
          owner: row.owner,
          folder: row.folder ?? null,
          callbackHost: row.callbackHost ?? null,
          outcome: row.outcome,
        })),
      };
    });
  }

  emailHandler(): EmailHandler<unknown, Docstore> {
    return {
      name: "McpEvents",
      handleEmailsEffect: (emails) =>
        Effect.forEach(emails, (email) => this.recordEmail(email), {
          concurrency: 1,
          discard: true,
        }),
    };
  }
}
