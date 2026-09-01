import { Logger } from "@micthiesen/mitools/logging";
import { Deferred, Effect, Exit, Fiber } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ParcelExtractionError } from "./effect.js";

const mocks = vi.hoisted(() => ({
  enqueue: vi.fn(),
  record: vi.fn(),
  extract: vi.fn(),
  submit: vi.fn(),
  recordSubmission: vi.fn(),
  reservation: undefined as
    | {
        trackingNumber: string;
        carrierCode: string;
        description: string;
        submittedAt: number;
        emailId: string;
        status?: "pending" | "submitted" | "rejected";
        attempts?: number;
      }
    | undefined,
}));

vi.mock("../email/retry.js", () => ({ enqueueEmailRetry: mocks.enqueue }));
vi.mock("../email/activity.js", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  recordEmailActivity: mocks.record,
}));
vi.mock("../email/activityLogs.js", () => ({
  withEmailLogCaptureEffect: (
    _key: string,
    _pipeline: string,
    run: () => Effect.Effect<void>,
  ) => run(),
}));
vi.mock("./filter/keywords.js", () => ({
  filterTrackingCandidateEffect: () =>
    Effect.succeed({ pass: true, reason: "test", admitTier: "rules" }),
}));
vi.mock("./extraction/extractDeliveries.js", () => ({
  extractDeliveriesEffect: mocks.extract,
}));
vi.mock("./carriers/carrierMap.js", () => ({
  getValidCarrierCodesEffect: () => Effect.succeed(new Set(["ups"])),
}));
vi.mock("./parcel/parcelApi.js", () => ({
  shouldTryNextCandidate: () => false,
  submitDeliveryEffect: (...args: unknown[]) =>
    Effect.sync(() => mocks.submit(...args)),
}));
vi.mock("./persistence.js", () => ({
  findNearDuplicateTracking: () => undefined,
  getAllTrackingNumbers: () => new Set(),
  getDeliverySubmission: () => mocks.reservation,
  hasSubmittedDelivery: () =>
    mocks.reservation !== undefined && mocks.reservation.status !== "pending",
  reserveDeliverySubmission: (row: typeof mocks.reservation) => {
    mocks.reservation = {
      ...row!,
      status: "pending",
      attempts: (mocks.reservation?.attempts ?? 0) + 1,
    };
    return mocks.reservation;
  },
  recordSubmittedDelivery: (row: typeof mocks.reservation) => {
    mocks.recordSubmission(row);
    mocks.reservation = row;
  },
}));

describe("DeliveryPipeline reliability", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.reservation = undefined;
  });

  it("durably queues admitted email after transient extraction failure", async () => {
    mocks.extract.mockReturnValueOnce(
      Effect.fail(
        new ParcelExtractionError({
          cause: new Error("model timeout"),
          transient: true,
        }),
      ),
    );
    const { DeliveryPipeline } = await import("./pipeline.js");
    const pipeline = new DeliveryPipeline(
      "parcel-key",
      new Logger("ParcelPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );

    await Effect.runPromise(
      pipeline.handleEmailsEffect([
        {
          id: "mail-2",
          subject: "Shipment",
          from: "merchant@example.com",
          textBody: "Tracking follows",
          links: [],
          receivedAt: "2026-09-01T00:00:00Z",
          attachments: [],
        },
      ]),
    );

    expect(mocks.enqueue).toHaveBeenCalledWith({
      pipeline: "ParcelTracker",
      emailId: "mail-2",
      reason: "Parcel extraction failed: model timeout",
    });
  });

  it("preserves interruption while delivery extraction is in progress", async () => {
    const started = await Effect.runPromise(Deferred.make<void>());
    mocks.extract.mockReturnValueOnce(
      Deferred.succeed(started, undefined).pipe(Effect.andThen(Effect.never)),
    );
    const { DeliveryPipeline } = await import("./pipeline.js");
    const pipeline = new DeliveryPipeline(
      "parcel-key",
      new Logger("ParcelPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );

    const exit = await Effect.runPromise(
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(
          pipeline.handleEmailsEffect([
            {
              id: "mail-interrupted",
              subject: "Shipment",
              from: "merchant@example.com",
              textBody: "Tracking follows",
              links: [],
              receivedAt: "2026-09-01T00:00:00Z",
              attachments: [],
            },
          ]),
        );
        yield* Deferred.await(started);
        yield* Fiber.interrupt(fiber);
        return yield* Fiber.await(fiber);
      }),
    );

    expect(Exit.hasInterrupts(exit)).toBe(true);
    expect(mocks.record).not.toHaveBeenCalled();
  });

  it("persists a replayable reservation before Parcel and retries an unacknowledged request", async () => {
    mocks.extract.mockReturnValue(
      Effect.succeed({
        deliveries: [
          {
            tracking_number: "1Z999AA10123456784",
            description: "Camera",
            carrier_candidates: ["ups"],
          },
        ],
        costCents: 0,
      }),
    );
    mocks.submit.mockReturnValue({ status: "success" });
    mocks.recordSubmission.mockImplementationOnce(() => {
      throw new Error("crash after Parcel accepted request");
    });
    const { DeliveryPipeline } = await import("./pipeline.js");
    const pipeline = new DeliveryPipeline(
      "parcel-key",
      new Logger("ParcelPipelineReliabilitySpec"),
      { getTriageCostCents: () => undefined } as never,
    );
    const shipment = {
      id: "mail-replay",
      subject: "Shipment",
      from: "merchant@example.com",
      textBody: "Tracking follows",
      links: [],
      receivedAt: "2026-09-01T00:00:00Z",
      attachments: [],
    };

    await Effect.runPromise(pipeline.handleEmailsEffect([shipment]));
    expect(mocks.reservation).toEqual(
      expect.objectContaining({
        trackingNumber: "1Z999AA10123456784",
        carrierCode: "ups",
        status: "pending",
        attempts: 1,
      }),
    );
    expect(mocks.submit).toHaveBeenCalledTimes(1);

    await Effect.runPromise(pipeline.handleEmailsEffect([shipment]));
    expect(mocks.submit).toHaveBeenCalledTimes(2);
    expect(mocks.reservation).toEqual(
      expect.objectContaining({ status: "submitted", attempts: 2 }),
    );
  });
});
