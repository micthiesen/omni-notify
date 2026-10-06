import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
  randomUUID,
} from "node:crypto";
import { Clock, Data, Effect, Queue, Semaphore } from "effect";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { FetchedEmail, EmailHandler } from "../../email/types.js";
import { isArchiveActionMessageEffect } from "../../email/archive/persistence.js";
import {
  canonicalEventArguments,
  EMAIL_RECEIVED,
  eventDefinition,
  type EventArguments,
  type EventData,
} from "./catalog.js";
import type { ExecutorEventAuthorizer } from "./executorAuth.js";
import {
  EventPersistence,
  type EventDelivery,
  type EventDeliveryFailure,
  type EventDeliveryWithhold,
  type EventRequestMethod,
  type EventSubscription,
} from "./persistence.js";
import {
  deliverWebhook,
  validateCallbackUrl,
  validateSigningSecret,
  verifyCallback,
} from "./webhook.js";

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
const PRUNE_INTERVAL_MS = 60 * 60_000;
/** Finished deliveries stay visible in diagnostics for a week. */
const DELIVERY_RETENTION_MS = 7 * 24 * 60 * 60_000;
/**
 * Receipts outlive IMAP's seven-day INTERNALDATE guard, so a replayed or moved
 * message is still recognized for as long as the transport can observe it.
 */
const RECEIPT_RETENTION_MS = 30 * 24 * 60 * 60_000;
const SUBSCRIPTION_RETENTION_MS = 7 * 24 * 60 * 60_000;

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
  arguments: Record<string, unknown>;
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
  arguments: Record<string, unknown>;
  delivery: { mode: "webhook"; url: string };
}

/** One source observation. Replays with the same receiptKey are dropped. */
export interface PublishInput {
  name: string;
  receiptKey: string;
  eventKey: string;
  timestamp: string;
  data: EventData;
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

/** A claimed delivery and the decrypted destination it is sent to. */
interface Claim {
  row: EventDelivery;
  url: string;
  secret: string;
  previousSecret?: string;
}

/**
 * Durable MCP Events outbox. Owner identity and encryption keys derive from the
 * MCP bearer token, never stored. State changes serialize on one short lock;
 * webhook and authorization I/O run outside it so a slow callback never delays
 * the email dispatch that publishes events.
 */
export class McpEventService {
  private readonly key: Buffer;
  private readonly keyId: string;
  private readonly owner: string;
  private readonly webhook: WebhookPort;
  private readonly stateLock = Semaphore.makeUnsafe(1);
  private readonly drainLock = Semaphore.makeUnsafe(1);
  private readonly authorizeOwner?: ExecutorEventAuthorizer;
  private readonly wake: Queue.Queue<void>;
  private lastPrunedAt = 0;

  static make(
    token: string,
    authorizeOwner?: ExecutorEventAuthorizer,
    webhook?: WebhookPort,
  ) {
    return Queue.sliding<void>(1).pipe(
      Effect.map((wake) => new McpEventService(token, wake, authorizeOwner, webhook)),
    );
  }

  private constructor(
    token: string,
    wake: Queue.Queue<void>,
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
    this.wake = wake;
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
    arguments: EventArguments;
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
    input?: { name: string; arguments: Record<string, unknown> },
    url?: string,
  ) {
    return Effect.gen(function* () {
      const definition = input && eventDefinition(input.name);
      const args = definition?.parseArguments(input?.arguments);
      yield* EventPersistence.recordRequest({
        id: randomUUID(),
        at: yield* Clock.currentTimeMillis,
        method,
        owner: ownerLabel(owner),
        ...(definition ? { name: definition.name } : {}),
        ...(args ? { arguments: args } : {}),
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
      this.recordRequest("events/subscribe", owner, outcome, input, input.delivery.url);
    // Authorization and the callback challenge are network I/O, so they run
    // before the state lock; the row is re-read and written under it.
    return Effect.gen({ self: this }, function* () {
      const tokenExpiresAt = yield* this.authorizePrincipal(owner, principal);
      const definition = eventDefinition(input.name);
      if (!definition) {
        return yield* new EventSubscriptionError({ reason: "invalid_event" });
      }
      const args = definition.parseArguments(input.arguments);
      if (!args) {
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
      const id = this.subscriptionId({
        owner,
        name: definition.name,
        arguments: args,
        url,
      });
      const known = yield* EventPersistence.subscription(id);
      const recentlyVerified =
        known?.owner === owner &&
        this.openSafe(known.encryptedSecret) === input.delivery.secret &&
        known.verifiedAt + VERIFY_CACHE_MS > (yield* Clock.currentTimeMillis);
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
      const verifiedAt = yield* Clock.currentTimeMillis;
      return yield* this.stateLock.withPermits(1)(
        Effect.gen({ self: this }, function* () {
          const now = yield* Clock.currentTimeMillis;
          const previous = yield* EventPersistence.subscription(id);
          const sameSecret =
            previous?.owner === owner &&
            this.openSafe(previous.encryptedSecret) === input.delivery.secret;
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
            name: definition.name,
            arguments: args,
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
            verifiedAt:
              recentlyVerified && sameSecret ? previous.verifiedAt : verifiedAt,
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
        }),
      );
    }).pipe(
      Effect.tap(({ refreshed }) => record(refreshed ? "refreshed" : "accepted")),
      Effect.tap(() => this.requestDrain()),
      Effect.tapError((error) =>
        record(error instanceof EventSubscriptionError ? error.reason : "error"),
      ),
      Effect.map(({ result }) => result),
    );
  }

  unsubscribe(input: UnsubscribeInput, principal?: EventPrincipal) {
    const owner = principal?.owner ?? this.owner;
    const record = (outcome: string) =>
      this.recordRequest(
        "events/unsubscribe",
        owner,
        outcome,
        input,
        input.delivery.url,
      );
    return Effect.gen({ self: this }, function* () {
      yield* this.authorizePrincipal(owner, principal);
      const definition = eventDefinition(input.name);
      const args = definition?.parseArguments(input.arguments);
      if (!definition || !args) return "not_found";
      let url: string;
      try {
        url = validateCallbackUrl(input.delivery.url).href;
      } catch {
        return "not_found";
      }
      const id = this.subscriptionId({
        owner,
        name: definition.name,
        arguments: args,
        url,
      });
      return yield* this.stateLock.withPermits(1)(
        Effect.gen(function* () {
          const prior = yield* EventPersistence.subscription(id);
          if (prior?.owner !== owner) return "not_found";
          yield* EventPersistence.deleteSubscription(id);
          return "removed";
        }),
      );
    }).pipe(
      Effect.tap(record),
      Effect.tapError((error) =>
        record(error instanceof EventSubscriptionError ? error.reason : "error"),
      ),
      Effect.as({}),
    );
  }

  /** Whether any current subscription would receive this event name. */
  hasActiveSubscription(name: string) {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      return (yield* EventPersistence.subscriptions()).some(
        (row) => row.keyId === this.keyId && row.name === name && row.expiresAt > now,
      );
    });
  }

  /**
   * Queue one event for every matching subscription, once per receiptKey.
   * The receipt and outbox rows commit atomically, then delivery starts.
   */
  publish(input: PublishInput) {
    return Effect.gen({ self: this }, function* () {
      const definition = eventDefinition(input.name);
      if (!definition) return false;
      const queued = yield* this.stateLock.withPermits(1)(
        Effect.gen({ self: this }, function* () {
          if (yield* EventPersistence.receipt(input.receiptKey)) return false;
          const now = yield* Clock.currentTimeMillis;
          const eventId = `evt_${digest(input.eventKey).slice(0, 40)}`;
          const deliveries: EventDelivery[] = (yield* EventPersistence.subscriptions())
            .filter(
              (subscription) =>
                subscription.keyId === this.keyId &&
                subscription.name === definition.name &&
                subscription.expiresAt > now &&
                definition.matches(subscription.arguments, input.data),
            )
            .map((subscription) => ({
              id: `${subscription.id}:${eventId}`,
              subscriptionId: subscription.id,
              owner: subscription.owner,
              subscriptionGeneration: subscription.generation,
              eventId,
              name: definition.name,
              timestamp: input.timestamp,
              data: input.data,
              attempts: 0,
              nextAttemptAt: now,
              status: "pending",
              createdAt: now,
              updatedAt: now,
            }));
          const folder = input.data.folder;
          yield* EventPersistence.commitReceiptAndDeliveries(
            {
              messageKey: input.receiptKey,
              name: definition.name,
              ...(typeof folder === "string" ? { folder } : {}),
              receivedAt: now,
            },
            deliveries,
          );
          return deliveries.length > 0;
        }),
      );
      if (queued) yield* this.requestDrain();
      return queued;
    });
  }

  /** Called before the IMAP cursor commits. Replayed polls preserve outbox IDs. */
  recordEmail(email: FetchedEmail) {
    return Effect.gen({ self: this }, function* () {
      const origin = email.origin;
      const folder = origin?.folder.toLowerCase();
      if (!origin || (folder !== "inbox" && folder !== "archive")) return;
      if (!email.messageId || !email.messageId.trim()) return;
      if (yield* isArchiveActionMessageEffect(email.messageId, origin)) return;
      const messageKey = digest(email.messageId);
      const timestamp = Number.isFinite(Date.parse(email.receivedAt))
        ? new Date(email.receivedAt).toISOString()
        : iso(yield* Clock.currentTimeMillis);
      // The receipt is per Message-ID, so a later move to Archive is not a
      // second arrival. The event key keeps IDs from before generic events.
      yield* this.publish({
        name: EMAIL_RECEIVED,
        receiptKey: messageKey,
        eventKey: `${messageKey}:${folder}`,
        timestamp,
        data: {
          messageId: email.messageId,
          folder,
          uidValidity: origin.uidValidity,
          uid: origin.uid,
        },
      });
    });
  }

  /** Ask the delivery worker for a pass; requests made during a pass coalesce. */
  requestDrain() {
    return Queue.offer(this.wake, undefined).pipe(Effect.asVoid);
  }

  /** Runs a delivery pass whenever an event is queued. Fork it in a scope. */
  deliveryWorker() {
    return Queue.take(this.wake).pipe(
      Effect.flatMap(() => this.drain()),
      Effect.catchCause(() => Effect.void),
      Effect.forever,
    );
  }

  /** Retry due rows after restart. Claim before network I/O; keep eventId stable. */
  drain() {
    return this.drainLock.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const now = yield* Clock.currentTimeMillis;
        const batch = (yield* EventPersistence.deliveries())
          .filter((row) => row.status === "pending" && row.nextAttemptAt <= now)
          .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt)
          .slice(0, MAX_DUE_PER_PASS);
        const authorization = yield* this.authorizeBatch(batch);
        const claims = yield* this.stateLock.withPermits(1)(
          this.claim(batch, authorization),
        );
        for (const claim of claims) yield* this.send(claim);
        yield* this.prune();
        return batch.length;
      }),
    );
  }

  /**
   * Checks each delegated subscription in the batch at most once. Executor
   * access tokens expire hourly, so a subscription without a token that
   * validates now is withheld until a refresh stores one or it ends.
   */
  private authorizeBatch(batch: EventDelivery[]) {
    return Effect.gen({ self: this }, function* () {
      const results = new Map<string, EventDeliveryWithhold | "ok">();
      for (const row of batch) {
        if (results.has(row.subscriptionId)) continue;
        const subscription = yield* EventPersistence.subscription(row.subscriptionId);
        if (!subscription?.owner.startsWith("executor:")) continue;
        const authorization =
          subscription.encryptedAuthorization &&
          this.openSafe(subscription.encryptedAuthorization);
        if (!authorization || !this.authorizeOwner) continue;
        const check = yield* this.authorizeOwner(
          subscription.owner,
          authorization,
        ).pipe(Effect.result);
        const now = yield* Clock.currentTimeMillis;
        results.set(
          row.subscriptionId,
          check._tag === "Failure"
            ? "authorization_unavailable"
            : check.success === null || check.success <= now
              ? "authorization_invalid"
              : "ok",
        );
      }
      return results;
    });
  }

  /** Fails, withholds, or claims each due row against current state. */
  private claim(
    batch: EventDelivery[],
    authorization: Map<string, EventDeliveryWithhold | "ok">,
  ) {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const claims: Claim[] = [];
      const withheld = new Set<string>();
      for (const due of batch) {
        if (withheld.has(due.subscriptionId)) continue;
        const row = yield* EventPersistence.delivery(due.id);
        if (!row || row.status !== "pending" || row.nextAttemptAt > now) continue;
        const fail = (failure: EventDeliveryFailure) =>
          EventPersistence.upsertDelivery({
            ...row,
            status: "failed",
            failure,
            withheld: undefined,
            updatedAt: now,
          });
        if (row.attempts >= MAX_ATTEMPTS) {
          yield* fail("attempts_exhausted");
          continue;
        }
        const subscription = yield* EventPersistence.subscription(row.subscriptionId);
        const definition = eventDefinition(row.name);
        if (
          !subscription ||
          !definition ||
          subscription.keyId !== this.keyId ||
          subscription.owner !== row.owner ||
          subscription.generation !== row.subscriptionGeneration ||
          subscription.expiresAt <= now ||
          subscription.name !== row.name ||
          !definition.matches(subscription.arguments, row.data)
        ) {
          yield* fail("subscription_inactive");
          continue;
        }
        if (subscription.owner.startsWith("executor:")) {
          const verdict = authorization.get(subscription.id);
          if (!verdict) {
            yield* fail("credentials_unavailable");
            continue;
          }
          if (verdict !== "ok") {
            withheld.add(subscription.id);
            yield* this.withhold(subscription.id, verdict, now);
            continue;
          }
        }
        const url = this.openSafe(subscription.encryptedUrl);
        const secret = this.openSafe(subscription.encryptedSecret);
        if (!url || !secret) {
          yield* fail("credentials_unavailable");
          continue;
        }
        const claimed: EventDelivery = {
          ...row,
          withheld: undefined,
          attempts: row.attempts + 1,
          nextAttemptAt: now + Math.min(6 * 60 * 60_000, 30_000 * 2 ** row.attempts),
          updatedAt: now,
        };
        yield* EventPersistence.upsertDelivery(claimed);
        claims.push({
          row: claimed,
          url,
          secret,
          previousSecret:
            subscription.previousSecretUntil &&
            subscription.previousSecretUntil > now &&
            subscription.encryptedPreviousSecret
              ? this.openSafe(subscription.encryptedPreviousSecret)
              : undefined,
        });
      }
      return claims;
    });
  }

  /** Defers all of a subscription's pending rows so it cannot crowd out others. */
  private withhold(subscriptionId: string, reason: EventDeliveryWithhold, now: number) {
    return Effect.gen(function* () {
      for (const held of yield* EventPersistence.deliveries()) {
        if (held.subscriptionId !== subscriptionId || held.status !== "pending") {
          continue;
        }
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
    });
  }

  private send(claim: Claim) {
    return Effect.gen({ self: this }, function* () {
      const { row } = claim;
      const response = yield* this.webhook
        .deliver({
          id: row.subscriptionId,
          url: claim.url,
          secret: claim.secret,
          previousSecret: claim.previousSecret,
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
      const exhausted = !delivered && !rejected && row.attempts >= MAX_ATTEMPTS;
      yield* this.stateLock.withPermits(1)(
        Effect.gen(function* () {
          yield* EventPersistence.upsertDelivery({
            ...row,
            status: delivered
              ? "delivered"
              : rejected || exhausted
                ? "failed"
                : "pending",
            lastStatus: status,
            lastError:
              response._tag === "Failure" ? response.failure.reason : undefined,
            failure: rejected
              ? "rejected"
              : exhausted
                ? "attempts_exhausted"
                : undefined,
            updatedAt: yield* Clock.currentTimeMillis,
          });
        }),
      );
    });
  }

  /** Bounds the outbox: finished deliveries, old receipts, long-ended subscriptions. */
  private prune() {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      if (now - this.lastPrunedAt < PRUNE_INTERVAL_MS) return;
      this.lastPrunedAt = now;
      yield* this.stateLock.withPermits(1)(
        Effect.gen(function* () {
          for (const row of yield* EventPersistence.deliveries()) {
            if (
              row.status !== "pending" &&
              row.updatedAt < now - DELIVERY_RETENTION_MS
            ) {
              yield* EventPersistence.deleteDelivery(row.id);
            }
          }
          for (const row of yield* EventPersistence.receipts()) {
            if (row.receivedAt < now - RECEIPT_RETENTION_MS) {
              yield* EventPersistence.deleteReceipt(row.messageKey);
            }
          }
          for (const row of yield* EventPersistence.subscriptions()) {
            if (row.expiresAt < now - SUBSCRIPTION_RETENTION_MS) {
              yield* EventPersistence.deleteSubscription(row.id);
            }
          }
        }),
      );
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
            name: row.name,
            arguments: row.arguments,
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
            name: row.name,
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
          name: row.name ?? null,
          arguments: row.arguments ?? null,
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
