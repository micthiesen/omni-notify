import { LogFile } from "@micthiesen/mitools/logfile";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { Cause, Effect } from "effect";
import {
  deriveItemsOutcome,
  recordEmailActivity,
  sumCostCents,
} from "../email/activity.js";
import { withEmailLogCaptureEffect } from "../email/activityLogs.js";
import { EmailRetryPersistence } from "../email/retry.js";
import type { EmailTriageService } from "../email/triage.js";
import type { EmailHandler, FetchedEmail } from "../email/types.js";
import config from "../utils/config.js";
import { selectValidCandidates } from "./carriers/candidates.js";
import { getValidCarrierCodesEffect } from "./carriers/carrierMap.js";
import {
  type ExtractedDelivery,
  extractDeliveriesEffect,
} from "./extraction/extractDeliveries.js";
import { filterTrackingCandidateEffect } from "./filter/keywords.js";
import type { AdmitTier } from "../email/activity.js";
import { shouldTryNextCandidate, submitDeliveryEffect } from "./parcel/parcelApi.js";
import { ParcelPersistenceError, parcelPersistenceEffect } from "./effect.js";
import {
  findNearDuplicateTracking,
  getDeliverySubmission,
  getAllTrackingNumbers,
  hasSubmittedDelivery,
  recordSubmittedDelivery,
  reserveDeliverySubmission,
} from "./persistence.js";

export class DeliveryPipeline implements EmailHandler<
  unknown,
  | import("@micthiesen/mitools/docstore").Docstore
  | import("@micthiesen/mitools/logging").Logger
> {
  public readonly name = "ParcelTracker";
  private logger: NamedLogger;
  private parcelApiKey: string;
  private triage: EmailTriageService;
  private rejectionLog?: LogFile;

  constructor(parcelApiKey: string, logger: NamedLogger, triage: EmailTriageService) {
    this.parcelApiKey = parcelApiKey;
    this.logger = logger;
    this.triage = triage;
  }

  public handleEmailsEffect(emails: FetchedEmail[]) {
    return Effect.gen({ self: this }, function* () {
      if (config.LOGS_PATH && !this.rejectionLog) {
        const dir = `${config.LOGS_PATH}/parcel-tracker`;
        this.rejectionLog = yield* LogFile.make(`${dir}/rejections.md`, "append");
      }
      const candidates = yield* Effect.forEach(emails, (email) =>
        filterTrackingCandidateEffect(email, this.logger, this.triage).pipe(
          Effect.flatMap((result) =>
            Effect.gen({ self: this }, function* () {
              if (result.pass) {
                yield* this.logger.info(
                  `Candidate (${result.reason}): "${email.subject}" from ${email.from}`,
                );
                return {
                  email,
                  admitReason: result.reason,
                  admitTier: result.admitTier as AdmitTier,
                };
              }
              yield* this.logger.info(
                `Skipped (${result.reason}): "${email.subject}" from ${email.from}`,
              );
              yield* recordEmailActivity({
                pipeline: this.name,
                email,
                outcome: "filtered",
                detail: result.reason,
                costCents: this.triage.getTriageCostCents(email.id),
              });
              return undefined;
            }),
          ),
        ),
      ).pipe(Effect.map((items) => items.filter((item) => item !== undefined)));

      yield* Effect.forEach(
        candidates,
        ({ email, admitReason, admitTier }) =>
          Effect.gen({ self: this }, function* () {
            const runLog = config.LOGS_PATH
              ? yield* LogFile.make(
                  `${config.LOGS_PATH}/parcel-tracker/${logTimestamp()}.md`,
                  "overwrite",
                )
              : undefined;
            // Triage cost only counts toward this row when triage is what admitted
            // it; the shared EmailTriageService memoizes per email, so the same
            // triage cost may also appear on CalendarEvents' row for this email —
            // acceptable for per-email transparency (see EmailTriageService docs).
            const triageCostCents =
              admitTier === "triage"
                ? this.triage.getTriageCostCents(email.id)
                : undefined;
            const program = this.processEmail(email, runLog).pipe(
              Effect.tap(({ results, costCents: extractionCostCents }) =>
                recordEmailActivity({
                  pipeline: this.name,
                  email,
                  outcome: deriveItemsOutcome(results.map((r) => r.ok)),
                  detail: results.length > 0 ? undefined : "no tracking numbers found",
                  admitReason,
                  admitTier,
                  costCents: sumCostCents([triageCostCents, extractionCostCents]),
                  items: results.length > 0 ? results.map((r) => r.line) : undefined,
                }),
              ),
              Effect.catch((error) =>
                Effect.gen({ self: this }, function* () {
                  yield* this.logger.error(
                    `Failed to process email "${email.subject}"`,
                    error.message,
                  );
                  yield* recordEmailActivity({
                    pipeline: this.name,
                    email,
                    outcome: "error",
                    detail: error.message,
                    admitReason,
                    admitTier,
                    costCents: sumCostCents([triageCostCents]),
                  });
                  if ("transient" in error && error.transient) {
                    yield* EmailRetryPersistence.enqueue({
                      pipeline: this.name,
                      emailId: email.id,
                      reason: error.message,
                    }).pipe(
                      Effect.mapError(
                        (cause) =>
                          new ParcelPersistenceError({
                            operation: "enqueue parcel email retry",
                            cause,
                          }),
                      ),
                    );
                  }
                }),
              ),
            );
            return yield* withEmailLogCaptureEffect(
              `${this.name}#${email.id}`,
              this.name,
              () => program,
            ).pipe(
              Effect.catchCause((cause) =>
                Cause.hasInterrupts(cause) ? Effect.interrupt : Effect.logError(cause),
              ),
            );
          }),
        { discard: true },
      );
    });
  }

  /**
   * Returns a short per-delivery result for each extracted delivery, plus
   * the extraction call's cost.
   */
  private processEmail(
    email: {
      id: string;
      subject: string;
      from: string;
      textBody: string;
      links: string[];
    },
    runLog?: LogFile,
  ) {
    return Effect.gen({ self: this }, function* () {
      yield* this.logger.info(
        `Extracting from: "${email.subject}" (from: ${email.from})`,
      );
      const { deliveries, costCents } = yield* extractDeliveriesEffect(
        {
          subject: email.subject,
          from: email.from,
          textBody: email.textBody,
          links: email.links,
        },
        this.logger,
        runLog,
      );

      if (deliveries.length === 0) {
        yield* this.logger.info(`No tracking numbers found in "${email.subject}"`);
        return { results: [], costCents };
      }

      yield* this.logger.info(
        `Found ${deliveries.length} delivery(ies) in "${email.subject}"`,
      );

      const results = yield* Effect.forEach(deliveries, (delivery) =>
        this.processDelivery(delivery, email.id),
      );
      return { results, costCents };
    });
  }

  /** Returns a short result line + success flag for the activity record. */
  private processDelivery(delivery: ExtractedDelivery, emailId: string) {
    return Effect.gen({ self: this }, function* () {
      const { tracking_number: trackingNumber, description } = delivery;
      const priorSubmission = yield* parcelPersistenceEffect(
        "read delivery reservation",
        () => getDeliverySubmission(trackingNumber),
      );

      // Dedup checks read persistence live so within-batch duplicates are caught
      if (
        yield* parcelPersistenceEffect("check submitted delivery", () =>
          hasSubmittedDelivery(trackingNumber),
        )
      ) {
        yield* this.logger.info(
          `Duplicate tracking number: ${trackingNumber} (skipping)`,
        );
        return { line: `${trackingNumber}: already submitted`, ok: true };
      }

      // Near-duplicate: the same shipment's number truncated differently by
      // another merchant email (e.g. P5253806501 vs P52538065)
      const knownTrackingNumbers = yield* parcelPersistenceEffect(
        "list submitted deliveries",
        () => getAllTrackingNumbers(),
      );
      const nearDuplicate = findNearDuplicateTracking(
        trackingNumber,
        knownTrackingNumbers,
      );
      if (nearDuplicate !== undefined) {
        yield* this.logger.info(
          `Near-duplicate tracking number: ${trackingNumber} matches known ${nearDuplicate} (skipping)`,
        );
        return {
          line: `${trackingNumber}: near-duplicate of ${nearDuplicate}, skipped`,
          ok: true,
        };
      }

      // Validate carrier candidates against the live Parcel carrier list
      const validCodes = yield* getValidCarrierCodesEffect(this.logger);
      if (!validCodes) {
        yield* this.logger.warn(
          `Carrier list unavailable, cannot validate candidates for ${trackingNumber}`,
        );
        return { line: `${trackingNumber}: carrier list unavailable`, ok: false };
      }

      const { valid: extractedCandidates, invalid } = selectValidCandidates(
        delivery.carrier_candidates,
        validCodes,
      );
      const candidates =
        priorSubmission?.status === "pending" &&
        validCodes.has(priorSubmission.carrierCode)
          ? [
              priorSubmission.carrierCode,
              ...extractedCandidates.filter(
                (code) => code !== priorSubmission.carrierCode,
              ),
            ]
          : extractedCandidates;
      if (invalid.length > 0) {
        yield* this.logger.warn(
          `Dropped invalid carrier candidate(s) [${invalid.join(", ")}] for ${trackingNumber}`,
        );
      }
      if (candidates.length === 0) {
        yield* this.logger.warn(
          `No valid carrier candidates for ${trackingNumber}, skipping`,
        );
        return { line: `${trackingNumber}: no valid carrier candidates`, ok: false };
      }

      yield* this.logger.info(
        `Carrier candidates for ${trackingNumber}: [${candidates.join(", ")}]`,
      );

      // Try candidates in ranked order; fall back on carrier-shaped rejections.
      // Dedup is only recorded on a terminal outcome (success or final rejection),
      // so a failed attempt never blocks the fallback candidates.
      for (const [index, carrierCode] of candidates.entries()) {
        const label = `${trackingNumber} (${carrierCode})`;
        const attempt = `${index + 1}/${candidates.length}`;

        const submittedAt = yield* Effect.clockWith((clock) => clock.currentTimeMillis);
        yield* parcelPersistenceEffect("reserve Parcel submission", () =>
          reserveDeliverySubmission({
            trackingNumber,
            carrierCode,
            description,
            submittedAt,
            emailId,
          }),
        );

        const result = yield* submitDeliveryEffect(
          { trackingNumber, carrierCode, description },
          this.parcelApiKey,
          this.logger,
          this.rejectionLog,
        );

        if (result.status === "success") {
          if (index > 0) {
            yield* this.logger.info(
              `Fallback candidate "${carrierCode}" succeeded for ${trackingNumber} (attempt ${attempt})`,
            );
          }
          const confirmed = yield* getDeliverySubmission(trackingNumber).pipe(
            Effect.mapError(
              (cause) =>
                new ParcelPersistenceError({
                  operation: "read delivery attempt",
                  cause,
                }),
            ),
          );
          yield* parcelPersistenceEffect("confirm Parcel submission", () =>
            recordSubmittedDelivery({
              trackingNumber,
              carrierCode,
              description,
              submittedAt,
              emailId,
              status: "submitted",
              attempts: confirmed?.attempts,
            }),
          );
          return { line: `${label}: submitted`, ok: true };
        }

        if (result.status === "error") {
          // Transient (network/5xx): don't burn remaining candidates or record
          // dedup; enqueue the email for a retry pass instead
          yield* this.logger.warn(`Failed to submit ${label}, will retry later`);
          yield* EmailRetryPersistence.enqueue({
            pipeline: this.name,
            emailId,
            reason: `Parcel submission network/5xx for ${trackingNumber}`,
          }).pipe(
            Effect.mapError(
              (cause) =>
                new ParcelPersistenceError({
                  operation: "enqueue parcel email retry",
                  cause,
                }),
            ),
          );
          return { line: `${label}: submission failed, will retry`, ok: false };
        }

        // Rejected
        const nextCandidate = candidates[index + 1];
        if (shouldTryNextCandidate(result) && nextCandidate !== undefined) {
          yield* this.logger.warn(
            `Parcel rejected ${label} with ${result.statusCode} (attempt ${attempt}), trying next candidate "${nextCandidate}"`,
          );
          continue;
        }

        // Terminal rejection: record to prevent retrying hopeless submissions
        yield* this.logger.warn(
          `Parcel rejected ${label} with ${result.statusCode} (attempt ${attempt}), recording to prevent retry`,
        );
        const rejected = yield* getDeliverySubmission(trackingNumber).pipe(
          Effect.mapError(
            (cause) =>
              new ParcelPersistenceError({ operation: "read delivery attempt", cause }),
          ),
        );
        yield* parcelPersistenceEffect("confirm Parcel rejection", () =>
          recordSubmittedDelivery({
            trackingNumber,
            carrierCode,
            description,
            submittedAt,
            emailId,
            status: "rejected",
            attempts: rejected?.attempts,
          }),
        );
        return {
          line: `${label}: rejected by Parcel (${result.statusCode})`,
          ok: false,
        };
      }

      // Unreachable: candidates is non-empty and every iteration returns or continues
      return yield* Effect.die(
        new Error(`No submission attempted for ${trackingNumber}`),
      );
    });
  }
}
