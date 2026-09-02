import { expect, layer } from "@effect/vitest";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Effect } from "effect";
import { TestClock } from "effect/testing";
import { EmailRetryEntity, EmailRetryPersistence } from "./retry.js";

layer(Docstore.layerMemory)("EmailRetryPersistence", (it) => {
  it.effect("uses the Effect clock and decodes persisted rows", () =>
    Effect.gen(function* () {
      yield* EmailRetryEntity.deleteAll();
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
      yield* EmailRetryEntity.deleteAll();
    }),
  );
});
