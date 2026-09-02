import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { Effect, Option } from "effect";
import {
  getDeliverySubmission,
  reserveDeliverySubmission,
  SubmittedDeliveryEntity,
} from "./persistence.js";

layer(Docstore.layerMemory)("Parcel submission reservations", (it) => {
  it.effect("increments attempts atomically across concurrent reservations", () =>
    Effect.gen(function* () {
      yield* SubmittedDeliveryEntity.deleteAll();
      const reservations = yield* Effect.all(
        Array.from({ length: 20 }, () =>
          reserveDeliverySubmission({
            trackingNumber: "1Z999AA10123456784",
            carrierCode: "ups",
            description: "Camera",
            submittedAt: 1_800_000_000_000,
            emailId: "email-1",
          }),
        ),
        { concurrency: "unbounded" },
      );

      expect(
        reservations.map(({ attempts }) => attempts).sort((a, b) => a! - b!),
      ).toEqual(Array.from({ length: 20 }, (_, index) => index + 1));
      expect((yield* getDeliverySubmission("1Z999AA10123456784"))?.attempts).toBe(20);
      expect(
        Option.isSome(
          yield* SubmittedDeliveryEntity.get({
            trackingNumber: "1Z999AA10123456784",
          }),
        ),
      ).toBe(true);
    }),
  );
});
