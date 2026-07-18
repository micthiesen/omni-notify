import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { JmapContext } from "./client.js";
import type { EmailHandler } from "./dispatcher.js";
import type { FetchedEmail } from "./emailFetcher.js";
import { EmailRetryEntity, enqueueEmailRetry } from "./retry.js";
import EmailRetryTask from "./retryTask.js";

vi.mock("./emailFetcher.js", () => ({
  fetchEmailById: vi.fn(async (): Promise<FetchedEmail> => fakeEmail),
}));

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "retrytask.spec.db",
  },
});

const fakeEmail: FetchedEmail = {
  id: "e1",
  subject: "Test",
  from: "a@b.com",
  textBody: "",
  links: [],
  receivedAt: new Date().toISOString(),
  attachments: [],
};

const logger = new Logger("Test");
const fakeCtx = {} as JmapContext;

function dueRow(pipeline: string, emailId: string, attempts: number): void {
  EmailRetryEntity.upsert({
    retryKey: `${pipeline}#${emailId}`,
    pipeline,
    emailId,
    reason: "test",
    attempts,
    nextAttemptAt: Date.now() - 1000,
    createdAt: Date.now() - 60_000,
  });
}

beforeAll(() => {
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  EmailRetryEntity.deleteAll();
});

describe("EmailRetryTask", () => {
  it("clears the row when the handler succeeds without re-enqueueing", async () => {
    dueRow("ParcelTracker", "e1", 1);
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmails: async () => {},
    };
    const task = new EmailRetryTask(
      () => ({ ctx: fakeCtx, handlers: new Map([["ParcelTracker", handler]]) }),
      logger,
    );

    await task.run();
    expect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })).toBeUndefined();
  });

  it("keeps the row when the handler re-enqueues without throwing", async () => {
    dueRow("ParcelTracker", "e1", 1);
    // Mirrors the pipelines: transient failure is swallowed and re-enqueued
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmails: async () => {
        enqueueEmailRetry({
          pipeline: "ParcelTracker",
          emailId: "e1",
          reason: "still down",
        });
      },
    };
    const task = new EmailRetryTask(
      () => ({ ctx: fakeCtx, handlers: new Map([["ParcelTracker", handler]]) }),
      logger,
    );

    await task.run();
    const row = EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" });
    expect(row).toBeDefined();
    expect(row?.attempts).toBe(2);
  });

  it("drops the row once re-enqueueing exceeds the attempt cap", async () => {
    dueRow("ParcelTracker", "e1", 5);
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmails: async () => {
        enqueueEmailRetry({
          pipeline: "ParcelTracker",
          emailId: "e1",
          reason: "still down",
        });
      },
    };
    const task = new EmailRetryTask(
      () => ({ ctx: fakeCtx, handlers: new Map([["ParcelTracker", handler]]) }),
      logger,
    );

    await task.run();
    expect(EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" })).toBeUndefined();
  });

  it("re-enqueues with bumped attempts when the handler throws", async () => {
    dueRow("ParcelTracker", "e1", 1);
    const handler: EmailHandler = {
      name: "ParcelTracker",
      handleEmails: async () => {
        throw new Error("boom");
      },
    };
    const task = new EmailRetryTask(
      () => ({ ctx: fakeCtx, handlers: new Map([["ParcelTracker", handler]]) }),
      logger,
    );

    await task.run();
    const row = EmailRetryEntity.get({ retryKey: "ParcelTracker#e1" });
    expect(row?.attempts).toBe(2);
  });
});
