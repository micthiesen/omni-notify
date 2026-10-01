import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { Effect, Option } from "effect";
import type { FetchedEmail } from "../../email/types.js";
import {
  queueArchiveActionEffect,
  updateArchiveActionEffect,
} from "../../email/archive/persistence.js";
import {
  EventDeliveryEntity,
  EventPersistence,
  EventReceiptEntity,
  EventSubscriptionEntity,
} from "./persistence.js";
import { EmailEventService } from "./service.js";

const secret = `whsec_${Buffer.alloc(32, 7).toString("base64")}`;
const url = "https://chatgpt.example.com/events/callback";

function email(messageId = "<one@example.test>", folder = "INBOX"): FetchedEmail {
  return {
    id: messageId,
    messageId,
    subject: "private fixture",
    from: "sender@example.test",
    textBody: "private body fixture",
    links: [],
    receivedAt: "2026-10-01T12:00:00.000Z",
    attachments: [],
    origin: { folder, uidValidity: "123", uid: 42 },
  };
}

function service(
  status = 204,
  authorizeOwner?: (owner: string, bearer: string) => Effect.Effect<boolean>,
) {
  const sent: string[] = [];
  const instance = new EmailEventService("test-omni-bearer", authorizeOwner, {
    verify: () => Effect.void,
    deliver: (input) => {
      sent.push(input.event.eventId);
      return Effect.succeed({ status });
    },
  });
  return { instance, sent };
}

const subscribeInput = (folder: "inbox" | "archive" = "inbox") => ({
  name: "email.received",
  arguments: { folder },
  delivery: { mode: "webhook" as const, url, secret },
});

layer(Docstore.layerMemory)("MCP Events durable lifecycle", (it) => {
  it.effect(
    "stores encrypted credentials and delivers each receipt once across restart",
    () =>
      Effect.gen(function* () {
        yield* EventSubscriptionEntity.deleteAll();
        yield* EventReceiptEntity.deleteAll();
        yield* EventDeliveryEntity.deleteAll();
        const first = service();
        const subscription = yield* first.instance.subscribe(subscribeInput());
        const stored = yield* EventPersistence.subscription(subscription.id);
        expect(stored?.encryptedSecret).not.toContain(secret);
        expect(stored?.encryptedUrl).not.toContain(url);

        yield* first.instance.recordEmail(email());
        yield* first.instance.recordEmail(email());
        const queued = yield* EventPersistence.deliveries();
        expect(queued).toHaveLength(1);
        expect(queued[0]?.data).toEqual({
          messageId: "<one@example.test>",
          folder: "inbox",
          uidValidity: "123",
          uid: 42,
        });
        expect(JSON.stringify(queued[0])).not.toContain("private body fixture");

        const restarted = service();
        expect(yield* restarted.instance.drain()).toBe(1);
        expect(restarted.sent).toEqual([queued[0]!.eventId]);
        expect(yield* restarted.instance.drain()).toBe(0);
        expect((yield* EventPersistence.delivery(queued[0]!.id))?.status).toBe(
          "delivered",
        );
      }),
  );

  it.effect("keeps first origin and does not backfill a new subscriber on replay", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service().instance;
      yield* events.recordEmail(email());
      expect(yield* EventPersistence.deliveries()).toHaveLength(0);
      yield* events.subscribe(subscribeInput());
      yield* events.recordEmail(email());
      yield* events.recordEmail(email("<one@example.test>", "Archive"));
      expect(yield* EventPersistence.deliveries()).toHaveLength(0);
      expect(yield* EventReceiptEntity.getAll()).toMatchObject([{ folder: "inbox" }]);
    }),
  );

  it.effect("unsubscribe cancels queued delivery and is idempotent", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service().instance;
      const subscribed = yield* events.subscribe(subscribeInput());
      yield* events.recordEmail(email());
      const input = {
        name: "email.received",
        arguments: { folder: "inbox" as const },
        delivery: { mode: "webhook" as const, url },
      };
      yield* events.unsubscribe(input);
      yield* events.unsubscribe(input);
      expect(
        Option.isNone(yield* EventSubscriptionEntity.get({ id: subscribed.id })),
      ).toBe(true);
      expect(yield* events.drain()).toBe(1);
      expect((yield* EventPersistence.deliveries())[0]?.status).toBe("failed");
    }),
  );

  it.effect("rejects revoked delegated owners before delivery", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      let active = true;
      const owner = `executor:${"a".repeat(64)}`;
      const events = service(204, () => Effect.succeed(active));
      const subscribed = yield* events.instance.subscribe(subscribeInput(), {
        owner,
        authorization: "Bearer delegated-fixture",
      });
      expect(
        (yield* EventPersistence.subscription(subscribed.id))?.encryptedAuthorization,
      ).not.toContain("delegated-fixture");
      yield* events.instance.recordEmail(email());
      active = false;
      yield* events.instance.drain();
      expect(events.sent).toHaveLength(0);
      expect((yield* EventPersistence.deliveries())[0]?.status).toBe("failed");
    }),
  );

  it.effect(
    "keeps token-rotated rows terminal without decrypting with the new key",
    () =>
      Effect.gen(function* () {
        yield* EventSubscriptionEntity.deleteAll();
        yield* EventReceiptEntity.deleteAll();
        yield* EventDeliveryEntity.deleteAll();
        const original = service().instance;
        yield* original.subscribe(subscribeInput());
        yield* original.recordEmail(email());
        const sent: string[] = [];
        const rotated = new EmailEventService("rotated-token", undefined, {
          verify: () => Effect.void,
          deliver: (input) => {
            sent.push(input.event.eventId);
            return Effect.succeed({ status: 204 });
          },
        });
        yield* rotated.drain();
        expect(sent).toHaveLength(0);
        expect((yield* EventPersistence.deliveries())[0]?.status).toBe("failed");
      }),
  );

  it.effect("keeps previous queued deliveries cancelled after resubscribe", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service();
      const first = yield* events.instance.subscribe(subscribeInput());
      yield* events.instance.recordEmail(email());
      const oldGeneration = (yield* EventPersistence.subscription(first.id))
        ?.generation;
      yield* events.instance.unsubscribe({
        name: "email.received",
        arguments: { folder: "inbox" },
        delivery: { mode: "webhook", url },
      });
      const second = yield* events.instance.subscribe(subscribeInput());
      expect(second.id).toBe(first.id);
      expect((yield* EventPersistence.subscription(second.id))?.generation).not.toBe(
        oldGeneration,
      );
      yield* events.instance.drain();
      expect(events.sent).toHaveLength(0);
      expect((yield* EventPersistence.deliveries())[0]?.status).toBe("failed");
    }),
  );

  it.effect("keeps original Inbox receipts and suppresses claimed move feedback", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service().instance;
      yield* events.subscribe(subscribeInput());
      yield* events.subscribe(subscribeInput("archive"));
      const messageId = "<archived@example.test>";
      const queued = yield* queueArchiveActionEffect("archive-test-action", {
        folder: "INBOX",
        uidValidity: "123",
        uid: 42,
        messageId,
      });
      yield* events.recordEmail(email(messageId, "INBOX"));
      expect(yield* EventPersistence.deliveries()).toHaveLength(1);
      yield* updateArchiveActionEffect(queued.actionId, "queued", "claimed");
      yield* events.recordEmail(email(messageId, "Archive"));
      expect(yield* EventPersistence.deliveries()).toHaveLength(1);
      const unseenId = "<archived-before-observation@example.test>";
      const unseen = yield* queueArchiveActionEffect("archive-unseen-action", {
        folder: "INBOX",
        uidValidity: "123",
        uid: 42,
        messageId: unseenId,
      });
      yield* updateArchiveActionEffect(unseen.actionId, "queued", "claimed");
      yield* events.recordEmail(email(unseenId, "Archive"));
      expect(yield* EventPersistence.deliveries()).toHaveLength(1);
    }),
  );

  it.effect("retries 429 and 5xx, but terminates 4xx and exhausted claims", () =>
    Effect.gen(function* () {
      for (const [status, expected] of [
        [400, "failed"],
        [429, "pending"],
        [503, "pending"],
      ] as const) {
        yield* EventSubscriptionEntity.deleteAll();
        yield* EventReceiptEntity.deleteAll();
        yield* EventDeliveryEntity.deleteAll();
        const events = service(status).instance;
        yield* events.subscribe(subscribeInput());
        yield* events.recordEmail(email());
        yield* events.drain();
        expect((yield* EventPersistence.deliveries())[0]?.status).toBe(expected);
      }
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service();
      yield* events.instance.subscribe(subscribeInput());
      yield* events.instance.recordEmail(email());
      const queued = (yield* EventPersistence.deliveries())[0]!;
      yield* EventPersistence.upsertDelivery({ ...queued, attempts: 8 });
      yield* events.instance.drain();
      expect(events.sent).toHaveLength(0);
      expect((yield* EventPersistence.delivery(queued.id))?.status).toBe("failed");
    }),
  );
});
