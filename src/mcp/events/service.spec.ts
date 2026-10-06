import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { Clock, Deferred, Effect, Fiber, Option } from "effect";
import { TestClock } from "effect/testing";
import type { FetchedEmail } from "../../email/types.js";
import type { McpRuntime } from "../runtime.js";
import { createEventTools } from "../tools/events.js";
import {
  queueArchiveActionEffect,
  updateArchiveActionEffect,
} from "../../email/archive/persistence.js";
import {
  EventDeliveryEntity,
  EventPersistence,
  EventReceiptEntity,
  EventRequestEntity,
  EventSubscriptionEntity,
} from "./persistence.js";
import { McpEventService } from "./service.js";

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

const TOKEN_LIFETIME_MS = 60 * 60_000;

/** Fixtures answer validity; a valid token expires an hour after the check. */
function service(
  status = 204,
  authorizeOwner?: (owner: string, bearer: string) => Effect.Effect<boolean, Error>,
) {
  const sent: string[] = [];
  const authorize =
    authorizeOwner &&
    ((owner: string, bearer: string) =>
      authorizeOwner(owner, bearer).pipe(
        Effect.flatMap((valid) =>
          valid
            ? Clock.currentTimeMillis.pipe(Effect.map((now) => now + TOKEN_LIFETIME_MS))
            : Effect.succeed(null),
        ),
      ));
  const instance = Effect.runSync(
    McpEventService.make("test-omni-bearer", authorize, {
      verify: () => Effect.void,
      deliver: (input) => {
        sent.push(input.event.eventId);
        return Effect.succeed({ status });
      },
    }),
  );
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

  it.effect("asks delegated subscribers to refresh by their token expiry", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      const owner = `executor:${"c".repeat(64)}`;
      const principal = { owner, authorization: "Bearer delegated-fixture" };
      const delegated = service(204, () => Effect.succeed(true)).instance;
      const now = yield* Clock.currentTimeMillis;

      const hourly = yield* delegated.subscribe(subscribeInput(), principal);
      expect(Date.parse(hourly.refreshBefore)).toBe(now + TOKEN_LIFETIME_MS - 60_000);
      expect((yield* EventPersistence.subscription(hourly.id))?.expiresAt).toBe(
        now + 24 * 60 * 60_000,
      );

      const brief = yield* delegated.subscribe(
        { ...subscribeInput("archive"), ttlMs: 10 * 60_000 },
        principal,
      );
      expect(Date.parse(brief.refreshBefore)).toBe(now + 10 * 60_000);

      const direct = yield* service().instance.subscribe(subscribeInput());
      expect(Date.parse(direct.refreshBefore)).toBe(now + 24 * 60 * 60_000);
    }),
  );

  it.effect("keeps refreshBefore within a nearly expired token's lifetime", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      const now = yield* Clock.currentTimeMillis;
      let expiry: number | null = now + 30_000;
      const events = Effect.runSync(
        McpEventService.make("test-omni-bearer", () => Effect.succeed(expiry), {
          verify: () => Effect.void,
          deliver: () => Effect.succeed({ status: 204 }),
        }),
      );
      const principal = {
        owner: `executor:${"d".repeat(64)}`,
        authorization: "Bearer delegated-fixture",
      };
      const soon = yield* events.subscribe(subscribeInput(), principal);
      expect(Date.parse(soon.refreshBefore)).toBe(now);

      expiry = now + 2 * TOKEN_LIFETIME_MS;
      const rotated = yield* events.subscribe(subscribeInput(), principal);
      expect(Date.parse(rotated.refreshBefore)).toBe(expiry - 60_000);

      const status = yield* events.status();
      expect(status.subscriptions[0]).toMatchObject({
        refreshBefore: new Date(expiry - 60_000).toISOString(),
        expiresAt: new Date(now + 24 * 60 * 60_000).toISOString(),
      });

      expiry = null;
      const expired = yield* events
        .subscribe(subscribeInput(), principal)
        .pipe(Effect.result);
      expect(expired._tag).toBe("Failure");
    }),
  );

  it.effect("withholds delegated delivery until the stored token validates", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      let valid = "Bearer delegated-fixture";
      const checked: string[] = [];
      const owner = `executor:${"a".repeat(64)}`;
      const events = service(204, (_, bearer) => {
        checked.push(bearer);
        return Effect.succeed(bearer === valid);
      });
      const subscribed = yield* events.instance.subscribe(subscribeInput(), {
        owner,
        authorization: valid,
      });
      expect(
        (yield* EventPersistence.subscription(subscribed.id))?.encryptedAuthorization,
      ).not.toContain("delegated-fixture");
      for (const id of ["<one@example.test>", "<two@example.test>", "<three@x.test>"])
        yield* events.instance.recordEmail(email(id));
      valid = "Bearer refreshed-fixture";
      checked.length = 0;
      expect(yield* events.instance.drain()).toBe(3);
      expect(checked).toEqual(["Bearer delegated-fixture"]);
      expect(yield* events.instance.drain()).toBe(0);
      yield* TestClock.adjust("14 minutes");
      expect(yield* events.instance.drain()).toBe(0);
      yield* TestClock.adjust("2 minutes");
      expect(yield* events.instance.drain()).toBe(3);
      expect(checked).toHaveLength(2);
      expect(events.sent).toHaveLength(0);
      const held = yield* EventPersistence.deliveries();
      expect(held).toHaveLength(3);
      for (const row of held) {
        expect(row).toMatchObject({
          status: "pending",
          withheld: "authorization_invalid",
          attempts: 0,
        });
      }

      yield* events.instance.subscribe(subscribeInput(), {
        owner,
        authorization: valid,
      });
      checked.length = 0;
      expect(yield* events.instance.drain()).toBe(3);
      expect(checked).toEqual(["Bearer refreshed-fixture"]);
      expect(events.sent).toHaveLength(3);
      for (const row of yield* EventPersistence.deliveries()) {
        expect(row.status).toBe("delivered");
        expect(row.withheld).toBeUndefined();
      }
    }),
  );

  it.effect("fails withheld events only when their subscription ends", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      let active = true;
      const owner = `executor:${"a".repeat(64)}`;
      const events = service(204, () => Effect.succeed(active));
      yield* events.instance.subscribe(subscribeInput(), {
        owner,
        authorization: "Bearer delegated-fixture",
      });
      yield* events.instance.recordEmail(email());
      active = false;
      yield* events.instance.drain();
      yield* TestClock.adjust("25 hours");
      yield* events.instance.drain();
      expect(events.sent).toHaveLength(0);
      const failed = (yield* EventPersistence.deliveries())[0];
      expect(failed).toMatchObject({
        status: "failed",
        failure: "subscription_inactive",
      });
      expect(failed?.withheld).toBeUndefined();
    }),
  );

  it.effect("holds a subscription briefly when Executor cannot authorize", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const owner = `executor:${"a".repeat(64)}`;
      const events = service(204, () => Effect.fail(new Error("Executor down")));
      const delegated = yield* events.instance
        .subscribe(subscribeInput(), {
          owner,
          authorization: "Bearer delegated-fixture",
        })
        .pipe(Effect.result);
      expect(delegated._tag).toBe("Failure");

      let available = true;
      const flaky = service(204, () =>
        available ? Effect.succeed(true) : Effect.fail(new Error("Executor down")),
      );
      yield* flaky.instance.subscribe(subscribeInput(), {
        owner,
        authorization: "Bearer delegated-fixture",
      });
      yield* flaky.instance.subscribe(subscribeInput("archive"));
      yield* flaky.instance.recordEmail(email("<inbox@example.test>"));
      yield* flaky.instance.recordEmail(email("<archive@example.test>", "Archive"));
      available = false;
      expect(yield* flaky.instance.drain()).toBe(2);
      expect(flaky.sent).toHaveLength(1);
      const rows = yield* EventPersistence.deliveries();
      expect(rows.find((row) => row.data.folder === "inbox")).toMatchObject({
        status: "pending",
        withheld: "authorization_unavailable",
        attempts: 0,
      });
      expect(rows.find((row) => row.data.folder === "archive")?.status).toBe(
        "delivered",
      );
      available = true;
      yield* TestClock.adjust("61 seconds");
      yield* flaky.instance.drain();
      expect(flaky.sent).toHaveLength(2);
    }),
  );

  it.effect("serves the status tool from the service without secrets", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      yield* EventRequestEntity.deleteAll();
      const base = { emailControls: {} } as unknown as McpRuntime;
      const run = (runtime: McpRuntime) =>
        createEventTools(runtime)[0]!.execute({}) as unknown as Effect.Effect<
          Record<string, unknown>,
          unknown,
          Docstore
        >;
      expect(yield* run(base)).toMatchObject({ enabled: false, subscriptions: [] });
      const events = service().instance;
      yield* events.subscribe(subscribeInput());
      yield* events.recordEmail(email("<tool@example.test>"));
      const status = yield* run({ ...base, events });
      expect(status).toMatchObject({
        enabled: true,
        subscriptionTotal: 1,
        subscriptions: [
          {
            name: "email.received",
            arguments: { folder: "inbox" },
            callbackHost: "chatgpt.example.com",
          },
        ],
        deliveries: { pending: 1, withheld: 0 },
      });
      expect(JSON.stringify(status)).not.toContain("<tool@example.test>");
      expect(JSON.stringify(status)).not.toContain(secret);
    }),
  );

  it.effect("records bounded event requests without credentials or paths", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventRequestEntity.deleteAll();
      const owner = `executor:${"b".repeat(64)}`;
      const principal = { owner, authorization: "Bearer delegated-fixture" };
      const events = service(204, () => Effect.succeed(true)).instance;
      yield* events.recordDiscovery(owner);
      yield* TestClock.adjust("1 second");
      yield* events.subscribe(subscribeInput(), principal);
      yield* TestClock.adjust("1 second");
      yield* events.subscribe(subscribeInput(), principal);
      yield* TestClock.adjust("1 second");
      const rejected = yield* events
        .subscribe(
          {
            ...subscribeInput("archive"),
            delivery: { mode: "webhook", url: "http://plain.example.com/x", secret },
          },
          principal,
        )
        .pipe(Effect.result);
      expect(rejected._tag).toBe("Failure");
      yield* TestClock.adjust("1 second");
      const removal = {
        name: "email.received",
        arguments: { folder: "inbox" as const },
        delivery: { mode: "webhook" as const, url },
      };
      yield* events.unsubscribe(removal, principal);
      yield* TestClock.adjust("1 second");
      yield* events.unsubscribe(removal, principal);

      const status = yield* events.status();
      expect(
        status.requests.map(({ method, outcome }) => `${method} ${outcome}`),
      ).toEqual([
        "events/unsubscribe not_found",
        "events/unsubscribe removed",
        "events/subscribe invalid_callback",
        "events/subscribe refreshed",
        "events/subscribe accepted",
        "events/list listed",
      ]);
      expect(status.requests[4]).toMatchObject({
        owner: `executor:${"b".repeat(12)}`,
        name: "email.received",
        arguments: { folder: "inbox" },
        callbackHost: "chatgpt.example.com",
      });
      const text = JSON.stringify(status);
      for (const hidden of [secret, "/events/callback", "delegated-fixture", owner]) {
        expect(text).not.toContain(hidden);
      }

      for (let index = 0; index < 35; index++) yield* events.recordDiscovery();
      expect(yield* EventPersistence.requests()).toHaveLength(30);
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
        const rotated = Effect.runSync(
          McpEventService.make("rotated-token", undefined, {
            verify: () => Effect.void,
            deliver: (input) => {
              sent.push(input.event.eventId);
              return Effect.succeed({ status: 204 });
            },
          }),
        );
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
        expect((yield* EventPersistence.deliveries())[0]).toMatchObject({
          status: expected,
          lastStatus: status,
          failure: expected === "failed" ? "rejected" : undefined,
        });
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
      expect(yield* EventPersistence.delivery(queued.id)).toMatchObject({
        status: "failed",
        failure: "attempts_exhausted",
      });
    }),
  );
  it.effect("publishes while a slow webhook is still in flight", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const release = yield* Deferred.make<void>();
      const started = yield* Deferred.make<void>();
      const events = yield* McpEventService.make("test-omni-bearer", undefined, {
        verify: () => Effect.void,
        deliver: () =>
          Deferred.succeed(started, undefined).pipe(
            Effect.andThen(Deferred.await(release)),
            Effect.as({ status: 204 }),
          ),
      });
      yield* events.subscribe(subscribeInput());
      yield* events.recordEmail(email("<slow@example.test>"));
      const drain = yield* Effect.forkChild(events.drain());
      yield* Deferred.await(started);
      // The webhook is blocked, yet the dispatcher's publish still commits.
      yield* events.recordEmail(email("<next@example.test>"));
      expect(yield* EventPersistence.deliveries()).toHaveLength(2);
      yield* Deferred.succeed(release, undefined);
      yield* Fiber.join(drain);
      const statuses = (yield* EventPersistence.deliveries()).map((row) => row.status);
      expect(statuses.sort()).toEqual(["delivered", "pending"]);
    }),
  );

  it.effect("delivers as soon as an event is published", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const delivered = yield* Deferred.make<string>();
      const events = yield* McpEventService.make("test-omni-bearer", undefined, {
        verify: () => Effect.void,
        deliver: (input) =>
          Deferred.succeed(delivered, input.event.eventId).pipe(
            Effect.as({ status: 204 }),
          ),
      });
      yield* Effect.forkChild(events.deliveryWorker());
      yield* events.subscribe(subscribeInput());
      yield* events.recordEmail(email("<fast@example.test>"));
      const eventId = yield* Deferred.await(delivered);
      expect((yield* EventPersistence.deliveries())[0]?.eventId).toBe(eventId);
    }),
  );

  it.effect("prunes finished deliveries, old receipts and ended subscriptions", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      yield* TestClock.adjust("1 hour");
      const events = service();
      yield* events.instance.subscribe({ ...subscribeInput(), ttlMs: 60_000 });
      yield* events.instance.recordEmail(email("<old@example.test>"));
      yield* events.instance.drain();
      yield* TestClock.adjust("8 days");
      yield* events.instance.drain();
      expect(yield* EventPersistence.deliveries()).toHaveLength(0);
      expect(yield* EventSubscriptionEntity.getAll()).toHaveLength(0);
      // Receipts outlive the IMAP guard, so a replay is still recognized.
      expect(yield* EventReceiptEntity.getAll()).toHaveLength(1);
      yield* TestClock.adjust("30 days");
      yield* events.instance.drain();
      expect(yield* EventReceiptEntity.getAll()).toHaveLength(0);
    }),
  );

  it.effect("delivers rows stored before events were generic", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service();
      const { id } = yield* events.instance.subscribe(subscribeInput());
      const stored = (yield* EventSubscriptionEntity.get({ id })).pipe(
        Option.getOrThrow,
      );
      const { arguments: _arguments, ...legacy } = stored as typeof stored & {
        arguments: unknown;
      };
      yield* EventSubscriptionEntity.upsert({ ...legacy, folder: "inbox" } as never);
      expect((yield* EventPersistence.subscription(id))?.arguments).toEqual({
        folder: "inbox",
      });
      yield* EventRequestEntity.upsert({
        id: "legacy-request",
        at: 1,
        method: "events/subscribe",
        owner: "direct",
        folder: "archive",
        outcome: "accepted",
      } as never);
      expect(
        (yield* EventPersistence.requests()).find((row) => row.id === "legacy-request"),
      ).toMatchObject({ arguments: { folder: "archive" } });
      // Refreshing the legacy row keeps its identity.
      expect((yield* events.instance.subscribe(subscribeInput())).id).toBe(id);
      yield* events.instance.recordEmail(email("<legacy@example.test>"));
      yield* events.instance.drain();
      expect(events.sent).toHaveLength(1);
    }),
  );

  it.effect("matches claude turn events to subscribed projects", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const events = service();
      const claude = (args: Record<string, unknown>) => ({
        name: "claude.session.turn_finished",
        arguments: args,
        delivery: { mode: "webhook" as const, url, secret },
      });
      const failure = yield* events.instance
        .subscribe(claude({ project: "../etc" }))
        .pipe(Effect.flip);
      expect(failure).toMatchObject({ reason: "invalid_arguments" });
      yield* events.instance.subscribe(claude({ project: "omni-notify" }));
      yield* events.instance.subscribe({
        ...claude({}),
        delivery: { ...claude({}).delivery, url: `${url}/all` },
      });
      expect(
        yield* events.instance.hasActiveSubscription("claude.session.turn_finished"),
      ).toBe(true);
      const turn = (project: string, revision: number) =>
        events.instance.publish({
          name: "claude.session.turn_finished",
          receiptKey: `turn:${project}:${revision}`,
          eventKey: `turn:${project}:${revision}`,
          timestamp: "2026-10-05T00:00:00.000Z",
          data: { sessionId: "s", id: "s1", project, status: "idle", revision },
        });
      yield* turn("omni-notify", 3);
      yield* turn("dotfiles", 4);
      yield* turn("dotfiles", 4);
      expect(yield* EventPersistence.deliveries()).toHaveLength(3);
    }),
  );
  it.effect("publishes while a subscriber's challenge is still in flight", () =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      const release = yield* Deferred.make<void>();
      const started = yield* Deferred.make<void>();
      let challenges = 0;
      const events = yield* McpEventService.make("test-omni-bearer", undefined, {
        verify: () =>
          ++challenges === 1
            ? Effect.void
            : Deferred.succeed(started, undefined).pipe(
                Effect.andThen(Deferred.await(release)),
              ),
        deliver: () => Effect.succeed({ status: 204 }),
      });
      yield* events.subscribe(subscribeInput());
      const slow = yield* Effect.forkChild(
        events.subscribe({
          ...subscribeInput("archive"),
          delivery: { mode: "webhook", url: `${url}/slow`, secret },
        }),
      );
      yield* Deferred.await(started);
      yield* events.recordEmail(email("<during@example.test>"));
      expect(yield* EventPersistence.deliveries()).toHaveLength(1);
      yield* Deferred.succeed(release, undefined);
      yield* Fiber.join(slow);
      expect(yield* EventSubscriptionEntity.getAll()).toHaveLength(2);
    }),
  );
});
