import { Docstore } from "@micthiesen/mitools/docstore";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EmailRetryEntity, EmailRetryPersistence } from "./retry.js";
import EmailRetryTask from "./retryTask.js";
import type { EmailHandler, EmailTransport, FetchedEmail } from "./types.js";

const runtime = ManagedRuntime.make(Layer.merge(Docstore.layerMemory, Logger.layer()));
const runEffect = runtime.runPromise.bind(runtime);

const fakeEmail: FetchedEmail = {
  id: "e1",
  subject: "Test",
  from: "a@b.com",
  textBody: "",
  links: [],
  receivedAt: new Date().toISOString(),
  attachments: [],
};

const logger = Logger.named("Test");
const fakeTransport = {
  fetchEmailByIdEffect: (): Effect.Effect<FetchedEmail> => Effect.succeed(fakeEmail),
} as unknown as EmailTransport;

function dueRow(pipeline: string, emailId: string, attempts: number) {
  return runEffect(
    EmailRetryEntity.upsert({
      retryKey: `${pipeline}#${emailId}`,
      pipeline,
      emailId,
      reason: "test",
      attempts,
      nextAttemptAt: Date.now() - 1000,
      createdAt: Date.now() - 60_000,
    }),
  );
}

beforeAll(() => {
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(async () => {
  await runEffect(EmailRetryEntity.deleteAll());
});

describe("EmailRetryTask", () => {
  it("clears the row when the handler succeeds without re-enqueueing", async () => {
    await dueRow("ParcelTracker", "e1", 1);
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmailsEffect: () => Effect.void,
    };
    const task = new EmailRetryTask(
      () => ({
        transport: fakeTransport,
        handlers: new Map([["ParcelTracker", handler]]),
      }),
      logger,
    );

    await runEffect(task.run);
    expect(
      Option.getOrUndefined(
        await runEffect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })),
      ),
    ).toBeUndefined();
  });

  it("keeps the row when the handler re-enqueues without throwing", async () => {
    await dueRow("ParcelTracker", "e1", 1);
    // Mirrors the pipelines: transient failure is swallowed and re-enqueued
    const handler: EmailHandler<unknown, Docstore> = {
      name: "ParcelTracker",
      handleEmailsEffect: () =>
        EmailRetryPersistence.enqueue({
          pipeline: "ParcelTracker",
          emailId: "e1",
          reason: "still down",
        }),
    };
    const task = new EmailRetryTask(
      () => ({
        transport: fakeTransport,
        handlers: new Map([["ParcelTracker", handler]]),
      }),
      logger,
    );

    await runEffect(task.run);
    const row = Option.getOrUndefined(
      await runEffect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })),
    );
    expect(row).toBeDefined();
    expect(row?.attempts).toBe(2);
  });

  it("drops the row once re-enqueueing exceeds the attempt cap", async () => {
    await dueRow("ParcelTracker", "e1", 5);
    const handler: EmailHandler<unknown, Docstore> = {
      name: "ParcelTracker",
      handleEmailsEffect: () =>
        EmailRetryPersistence.enqueue({
          pipeline: "ParcelTracker",
          emailId: "e1",
          reason: "still down",
        }),
    };
    const task = new EmailRetryTask(
      () => ({
        transport: fakeTransport,
        handlers: new Map([["ParcelTracker", handler]]),
      }),
      logger,
    );

    await runEffect(task.run);
    expect(
      Option.getOrUndefined(
        await runEffect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })),
      ),
    ).toBeUndefined();
  });

  it("drops a permanent handler failure instead of retrying it", async () => {
    await dueRow("ParcelTracker", "e1", 1);
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmailsEffect: () => Effect.fail(new Error("boom")),
    };
    const task = new EmailRetryTask(
      () => ({
        transport: fakeTransport,
        handlers: new Map([["ParcelTracker", handler]]),
      }),
      logger,
    );

    await runEffect(task.run);
    expect(
      Option.getOrUndefined(
        await runEffect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })),
      ),
    ).toBeUndefined();
  });

  it("continues the pass when fetching one due email fails", async () => {
    await dueRow("ParcelTracker", "e1", 0);
    await dueRow("ParcelTracker", "e2", 0);
    const handled: string[] = [];
    const transport = {
      fetchEmailByIdEffect: (id: string) =>
        id === "e1"
          ? Effect.fail(new Error("temporary transport failure"))
          : Effect.succeed({ ...fakeEmail, id }),
    } as unknown as EmailTransport;
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmailsEffect: (emails) =>
        Effect.sync(() => {
          handled.push(...emails.map((email) => email.id));
        }),
    };
    const task = new EmailRetryTask(
      () => ({
        transport,
        handlers: new Map([["ParcelTracker", handler]]),
      }),
      logger,
    );

    await runEffect(task.run);

    expect(handled).toEqual(["e2"]);
    expect(
      Option.getOrUndefined(
        await runEffect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })),
      )?.attempts,
    ).toBe(1);
    expect(
      Option.getOrUndefined(
        await runEffect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e2" })),
      ),
    ).toBeUndefined();
  });
});
