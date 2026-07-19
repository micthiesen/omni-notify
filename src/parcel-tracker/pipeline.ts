import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import {
  type AdmitTier,
  deriveItemsOutcome,
  recordEmailActivity,
  sumCostCents,
} from "../jmap/activity.js";
import { withEmailLogCapture } from "../jmap/activityLogs.js";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { FetchedEmail } from "../jmap/emailFetcher.js";
import { enqueueEmailRetry } from "../jmap/retry.js";
import type { EmailTriageService } from "../jmap/triage.js";
import config from "../utils/config.js";
import { selectValidCandidates } from "./carriers/candidates.js";
import { getValidCarrierCodes } from "./carriers/carrierMap.js";
import {
  type ExtractedDelivery,
  extractDeliveries,
} from "./extraction/extractDeliveries.js";
import { filterTrackingCandidate } from "./filter/keywords.js";
import { shouldTryNextCandidate, submitDelivery } from "./parcel/parcelApi.js";
import {
  findNearDuplicateTracking,
  getAllTrackingNumbers,
  hasSubmittedDelivery,
  recordSubmittedDelivery,
} from "./persistence.js";

/** Per-delivery result: the activity item line + whether the item succeeded. */
interface DeliveryResult {
  line: string;
  ok: boolean;
}

export class DeliveryPipeline implements EmailHandler {
  public readonly name = "ParcelTracker";
  private logger: Logger;
  private parcelApiKey: string;
  private triage: EmailTriageService;
  private rejectionLog?: LogFile;

  constructor(parcelApiKey: string, logger: Logger, triage: EmailTriageService) {
    this.parcelApiKey = parcelApiKey;
    this.logger = logger;
    this.triage = triage;

    if (config.LOGS_PATH) {
      const dir = `${config.LOGS_PATH}/parcel-tracker`;
      this.rejectionLog = new LogFile(`${dir}/rejections.md`, "append");
    }
  }

  async handleEmails(emails: FetchedEmail[]): Promise<void> {
    // Filter candidates. Each email is guarded individually: a throw here
    // must not reject the whole batch, because the dispatcher advances the
    // JMAP cursor regardless and the other emails would be lost silently.
    const candidates: {
      email: FetchedEmail;
      admitReason: string;
      admitTier: AdmitTier;
    }[] = [];
    for (const email of emails) {
      let result: Awaited<ReturnType<typeof filterTrackingCandidate>>;
      try {
        result = await filterTrackingCandidate(email, this.logger, this.triage);
      } catch (error) {
        this.logger.error(
          `Filter failed for "${email.subject}"`,
          (error as Error).message,
        );
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "error",
          detail: `filter failed: ${(error as Error).message}`,
        });
        continue;
      }
      if (result.pass) {
        this.logger.info(
          `Candidate (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        candidates.push({
          email,
          admitReason: result.reason,
          admitTier: result.admitTier,
        });
      } else {
        this.logger.info(
          `Skipped (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "filtered",
          detail: result.reason,
          // A triage-rejected email still incurred a paid LLM call; attribute
          // it (null when a cheaper tier rejected before triage ran).
          costCents: this.triage.getTriageCostCents(email.id),
        });
      }
    }

    // Process each candidate, capturing its log lines for the activity UI.
    // Dedup (exact + near-duplicate) happens per delivery after extraction,
    // reading persistence live so within-batch duplicates are caught too.
    for (const { email, admitReason, admitTier } of candidates) {
      const runLog = config.LOGS_PATH
        ? new LogFile(
            `${config.LOGS_PATH}/parcel-tracker/${logTimestamp()}.md`,
            "overwrite",
          )
        : undefined;
      // Triage cost only counts toward this row when triage is what admitted
      // it; the shared EmailTriageService memoizes per email, so the same
      // triage cost may also appear on CalendarEvents' row for this email —
      // acceptable for per-email transparency (see EmailTriageService docs).
      const triageCostCents =
        admitTier === "triage" ? this.triage.getTriageCostCents(email.id) : undefined;
      await withEmailLogCapture(`${this.name}#${email.id}`, this.name, async () => {
        try {
          const { results, costCents: extractionCostCents } = await this.processEmail(
            email,
            runLog,
          );
          recordEmailActivity({
            pipeline: this.name,
            email,
            outcome: deriveItemsOutcome(results.map((r) => r.ok)),
            detail: results.length > 0 ? undefined : "no tracking numbers found",
            admitReason,
            admitTier,
            costCents: sumCostCents([triageCostCents, extractionCostCents]),
            items: results.length > 0 ? results.map((r) => r.line) : undefined,
          });
        } catch (error) {
          this.logger.error(
            `Failed to process email "${email.subject}"`,
            (error as Error).message,
          );
          recordEmailActivity({
            pipeline: this.name,
            email,
            outcome: "error",
            detail: (error as Error).message,
            admitReason,
            admitTier,
            costCents: sumCostCents([triageCostCents]),
          });
          // Continue with other emails
        }
      });
    }
  }

  /**
   * Returns a short per-delivery result for each extracted delivery, plus
   * the extraction call's cost.
   */
  private async processEmail(
    email: {
      id: string;
      subject: string;
      from: string;
      textBody: string;
      links: string[];
    },
    runLog?: LogFile,
  ): Promise<{ results: DeliveryResult[]; costCents: number | null }> {
    this.logger.info(`Extracting from: "${email.subject}" (from: ${email.from})`);

    const { deliveries, costCents } = await extractDeliveries(
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
      this.logger.info(`No tracking numbers found in "${email.subject}"`);
      return { results: [], costCents };
    }

    this.logger.info(`Found ${deliveries.length} delivery(ies) in "${email.subject}"`);

    const results: DeliveryResult[] = [];
    for (const delivery of deliveries) {
      results.push(await this.processDelivery(delivery, email.id));
    }
    return { results, costCents };
  }

  /** Returns a short result line + success flag for the activity record. */
  private async processDelivery(
    delivery: ExtractedDelivery,
    emailId: string,
  ): Promise<DeliveryResult> {
    const { tracking_number: trackingNumber, description } = delivery;

    // Dedup checks read persistence live so within-batch duplicates are caught
    if (hasSubmittedDelivery(trackingNumber)) {
      this.logger.info(`Duplicate tracking number: ${trackingNumber} (skipping)`);
      return { line: `${trackingNumber}: already submitted`, ok: true };
    }

    // Near-duplicate: the same shipment's number truncated differently by
    // another merchant email (e.g. P5253806501 vs P52538065)
    const nearDuplicate = findNearDuplicateTracking(
      trackingNumber,
      getAllTrackingNumbers(),
    );
    if (nearDuplicate !== undefined) {
      this.logger.info(
        `Near-duplicate tracking number: ${trackingNumber} matches known ${nearDuplicate} (skipping)`,
      );
      return {
        line: `${trackingNumber}: near-duplicate of ${nearDuplicate}, skipped`,
        ok: true,
      };
    }

    // Validate carrier candidates against the live Parcel carrier list
    const validCodes = await getValidCarrierCodes(this.logger);
    if (!validCodes) {
      this.logger.warn(
        `Carrier list unavailable, cannot validate candidates for ${trackingNumber}`,
      );
      return { line: `${trackingNumber}: carrier list unavailable`, ok: false };
    }

    const { valid: candidates, invalid } = selectValidCandidates(
      delivery.carrier_candidates,
      validCodes,
    );
    if (invalid.length > 0) {
      this.logger.warn(
        `Dropped invalid carrier candidate(s) [${invalid.join(", ")}] for ${trackingNumber}`,
      );
    }
    if (candidates.length === 0) {
      this.logger.warn(`No valid carrier candidates for ${trackingNumber}, skipping`);
      return { line: `${trackingNumber}: no valid carrier candidates`, ok: false };
    }

    this.logger.info(
      `Carrier candidates for ${trackingNumber}: [${candidates.join(", ")}]`,
    );

    // Try candidates in ranked order; fall back on carrier-shaped rejections.
    // Dedup is only recorded on a terminal outcome (success or final rejection),
    // so a failed attempt never blocks the fallback candidates.
    for (const [index, carrierCode] of candidates.entries()) {
      const label = `${trackingNumber} (${carrierCode})`;
      const attempt = `${index + 1}/${candidates.length}`;

      const result = await submitDelivery(
        { trackingNumber, carrierCode, description },
        this.parcelApiKey,
        this.logger,
        this.rejectionLog,
      );

      if (result.status === "success") {
        if (index > 0) {
          this.logger.info(
            `Fallback candidate "${carrierCode}" succeeded for ${trackingNumber} (attempt ${attempt})`,
          );
        }
        recordSubmittedDelivery({
          trackingNumber,
          carrierCode,
          description,
          submittedAt: Date.now(),
          emailId,
        });
        return { line: `${label}: submitted`, ok: true };
      }

      if (result.status === "error") {
        // Transient (network/5xx): don't burn remaining candidates or record
        // dedup; enqueue the email for a retry pass instead
        this.logger.warn(`Failed to submit ${label}, will retry later`);
        enqueueEmailRetry({
          pipeline: this.name,
          emailId,
          reason: `Parcel submission network/5xx for ${trackingNumber}`,
        });
        return { line: `${label}: submission failed, will retry`, ok: false };
      }

      // Rejected
      const nextCandidate = candidates[index + 1];
      if (shouldTryNextCandidate(result) && nextCandidate !== undefined) {
        this.logger.warn(
          `Parcel rejected ${label} with ${result.statusCode} (attempt ${attempt}), trying next candidate "${nextCandidate}"`,
        );
        continue;
      }

      // Terminal rejection: record to prevent retrying hopeless submissions
      this.logger.warn(
        `Parcel rejected ${label} with ${result.statusCode} (attempt ${attempt}), recording to prevent retry`,
      );
      recordSubmittedDelivery({
        trackingNumber,
        carrierCode,
        description,
        submittedAt: Date.now(),
        emailId,
      });
      return { line: `${label}: rejected by Parcel (${result.statusCode})`, ok: false };
    }

    // Unreachable: candidates is non-empty and every iteration returns or continues
    throw new Error(`No submission attempted for ${trackingNumber}`);
  }
}
