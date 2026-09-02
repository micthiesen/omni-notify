import { describe, expect, it } from "@effect/vitest";
import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { TestClock } from "effect/testing";
import { EmailRetryEntity, EmailRetryPersistence } from "./retry.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "email-retry-effect.spec.db",
  },
});

describe("EmailRetryPersistence", () => {
  it.effect("uses the Effect clock and decodes persisted rows", () =>
    Effect.gen(function* () {
      EmailRetryEntity.deleteAll();
      yield* TestClock.setTime(1_800_000_000_000);

      yield* EmailRetryPersistence.enqueue({
        pipeline: "ParcelTracker",
        emailId: "email-1",
        reason: "service unavailable",
      });
      const [row] = yield* EmailRetryPersistence.getAll();

      expect(row).toMatchObject({
        retryKey: "ParcelTracker#email-1",
        attempts: 0,
        createdAt: 1_800_000_000_000,
        nextAttemptAt: 1_800_001_800_000,
      });
      EmailRetryEntity.deleteAll();
    }),
  );
});
