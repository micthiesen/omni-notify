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
import {
  EventPersistence,
  type EventDelivery,
  type EventFolder,
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

/** Owner identity and encryption keys derive from the MCP bearer token, never stored. */
export class EmailEventService {
  private readonly key: Buffer;
  private readonly keyId: string;
  private readonly owner: string;
  private readonly webhook: WebhookPort;
  private readonly semaphore = Semaphore.makeUnsafe(1);
  private readonly authorizeOwner?: (
    owner: string,
    authorization: string,
  ) => Effect.Effect<boolean, Error>;

  constructor(
    token: string,
    authorizeOwner?: (
      owner: string,
      authorization: string,
    ) => Effect.Effect<boolean, Error>,
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

  subscribe(input: SubscribeInput, principal?: EventPrincipal) {
    return this.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const owner = principal?.owner ?? this.owner;
        if (
          principal &&
          (!this.authorizeOwner ||
            !(yield* this.authorizeOwner(owner, principal.authorization)))
        ) {
          return yield* new EventSubscriptionError({ reason: "invalid_principal" });
        }
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
          expiresAt: now + ttl,
          verifiedAt: recentlyVerified ? previous.verifiedAt : now,
        };
        yield* EventPersistence.upsertSubscription(row);
        return {
          id,
          refreshBefore: new Date(row.expiresAt).toISOString(),
          cursor: null,
          truncated: false,
        };
      }),
    );
  }

  unsubscribe(input: UnsubscribeInput, principal?: EventPrincipal) {
    return this.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        const owner = principal?.owner ?? this.owner;
        if (
          principal &&
          (!this.authorizeOwner ||
            !(yield* this.authorizeOwner(owner, principal.authorization)))
        ) {
          return yield* new EventSubscriptionError({ reason: "invalid_principal" });
        }
        if (input.name !== EVENT_NAME) return {};
        if (input.arguments.folder !== "inbox" && input.arguments.folder !== "archive")
          return {};
        let url: string;
        try {
          url = validateCallbackUrl(input.delivery.url).href;
        } catch {
          return {};
        }
        const id = this.subscriptionId({ ...input, owner, url });
        const prior = yield* EventPersistence.subscription(id);
        if (prior?.owner === owner) yield* EventPersistence.deleteSubscription(id);
        return {};
      }),
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
        const due = (yield* EventPersistence.deliveries())
          .filter((row) => row.status === "pending" && row.nextAttemptAt <= now)
          .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt)
          .slice(0, MAX_DUE_PER_PASS);
        for (const row of due) yield* this.deliverOne(row);
        return due.length;
      }),
    );
  }

  private deliverOne(row: EventDelivery) {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const subscription = yield* EventPersistence.subscription(row.subscriptionId);
      if (
        row.attempts >= MAX_ATTEMPTS ||
        !subscription ||
        subscription.keyId !== this.keyId ||
        subscription.owner !== row.owner ||
        subscription.generation !== row.subscriptionGeneration ||
        subscription.expiresAt <= now ||
        subscription.folder !== row.data.folder
      ) {
        yield* EventPersistence.upsertDelivery({
          ...row,
          status: "failed",
          updatedAt: now,
        });
        return;
      }
      if (subscription.owner.startsWith("executor:")) {
        const authorization =
          subscription.encryptedAuthorization &&
          this.openSafe(subscription.encryptedAuthorization);
        if (
          !authorization ||
          !this.authorizeOwner ||
          !(yield* this.authorizeOwner(subscription.owner, authorization))
        ) {
          yield* EventPersistence.upsertDelivery({
            ...row,
            status: "failed",
            updatedAt: now,
          });
          return;
        }
      }
      const url = this.openSafe(subscription.encryptedUrl);
      const secret = this.openSafe(subscription.encryptedSecret);
      if (!url || !secret) {
        yield* EventPersistence.upsertDelivery({
          ...row,
          status: "failed",
          updatedAt: now,
        });
        return;
      }
      const claimed: EventDelivery = {
        ...row,
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
      yield* EventPersistence.upsertDelivery({
        ...claimed,
        status:
          status !== undefined && status >= 200 && status < 300
            ? "delivered"
            : (status !== undefined &&
                  status >= 300 &&
                  status < 500 &&
                  status !== 408 &&
                  status !== 429) ||
                claimed.attempts >= MAX_ATTEMPTS
              ? "failed"
              : "pending",
        lastStatus: status,
        updatedAt: yield* Clock.currentTimeMillis,
      });
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
