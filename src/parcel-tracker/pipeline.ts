import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { recordEmailActivity } from "../jmap/activity.js";
import { withEmailLogCapture } from "../jmap/activityLogs.js";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { FetchedEmail } from "../jmap/emailFetcher.js";
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
  getRecentTrackingNumbers,
  hasSubmittedDelivery,
  recordSubmittedDelivery,
} from "./persistence.js";

export class DeliveryPipeline implements EmailHandler {
  public readonly name = "ParcelTracker";
  private logger: Logger;
  private parcelApiKey: string;
  private rejectionLog?: LogFile;

  constructor(parcelApiKey: string, logger: Logger) {
    this.parcelApiKey = parcelApiKey;
    this.logger = logger;

    if (config.LOGS_PATH) {
      const dir = `${config.LOGS_PATH}/parcel-tracker`;
      this.rejectionLog = new LogFile(`${dir}/rejections.md`, "append");
    }
  }

  async handleEmails(emails: FetchedEmail[]): Promise<void> {
    // Filter candidates
    const candidates = [];
    for (const email of emails) {
      const result = await filterTrackingCandidate(
        { from: email.from, subject: email.subject, textBody: email.textBody },
        this.logger,
      );
      if (result.pass) {
        this.logger.info(
          `Candidate (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        candidates.push(email);
      } else {
        this.logger.info(
          `Skipped (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "filtered",
          detail: result.reason,
        });
      }
    }

    // Pre-filter: skip emails that mention already-submitted tracking numbers
    const knownNumbers = getRecentTrackingNumbers();
    const newCandidates = candidates.filter((email) => {
      const text = `${email.subject} ${email.textBody}`;
      const match = [...knownNumbers].find((num) => text.includes(num));
      if (match) {
        this.logger.info(
          `Skipping "${email.subject}" — contains known tracking number ${match}`,
        );
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "skipped",
          detail: `contains known tracking number ${match}`,
        });
        return false;
      }
      return true;
    });

    if (newCandidates.length < candidates.length) {
      this.logger.info(
        `Skipped ${candidates.length - newCandidates.length} email(s) with known tracking numbers`,
      );
    }

    // Process each candidate, capturing its log lines for the activity UI
    for (const email of newCandidates) {
      const runLog = config.LOGS_PATH
        ? new LogFile(
            `${config.LOGS_PATH}/parcel-tracker/${logTimestamp()}.md`,
            "overwrite",
          )
        : undefined;
      await withEmailLogCapture(`${this.name}#${email.id}`, this.name, async () => {
        try {
          const items = await this.processEmail(email, runLog);
          recordEmailActivity({
            pipeline: this.name,
            email,
            outcome: items.length > 0 ? "processed" : "no_matches",
            detail: items.length > 0 ? undefined : "no tracking numbers found",
            items: items.length > 0 ? items : undefined,
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
          });
          // Continue with other emails
        }
      });
    }
  }

  /** Returns a short per-delivery result line for each extracted delivery. */
  private async processEmail(
    email: {
      id: string;
      subject: string;
      from: string;
      textBody: string;
    },
    runLog?: LogFile,
  ): Promise<string[]> {
    this.logger.info(`Extracting from: "${email.subject}" (from: ${email.from})`);

    const deliveries = await extractDeliveries(
      { subject: email.subject, from: email.from, textBody: email.textBody },
      this.logger,
      runLog,
    );

    if (deliveries.length === 0) {
      this.logger.info(`No tracking numbers found in "${email.subject}"`);
      return [];
    }

    this.logger.info(`Found ${deliveries.length} delivery(ies) in "${email.subject}"`);

    const results: string[] = [];
    for (const delivery of deliveries) {
      results.push(await this.processDelivery(delivery, email.id));
    }
    return results;
  }

  /** Returns a short result line for the activity record. */
  private async processDelivery(
    delivery: ExtractedDelivery,
    emailId: string,
  ): Promise<string> {
    const { tracking_number: trackingNumber, description } = delivery;

    // Dedup check
    if (hasSubmittedDelivery(trackingNumber)) {
      this.logger.info(`Duplicate tracking number: ${trackingNumber} (skipping)`);
      return `${trackingNumber}: already submitted`;
    }

    // Validate carrier candidates against the live Parcel carrier list
    const validCodes = await getValidCarrierCodes(this.logger);
    if (!validCodes) {
      this.logger.warn(
        `Carrier list unavailable, cannot validate candidates for ${trackingNumber}`,
      );
      return `${trackingNumber}: carrier list unavailable`;
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
      return `${trackingNumber}: no valid carrier candidates`;
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
        return `${label}: submitted`;
      }

      if (result.status === "error") {
        // Transient (network/5xx): don't burn remaining candidates or record dedup
        this.logger.warn(`Failed to submit ${label}, will retry later`);
        return `${label}: submission failed, will retry`;
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
      return `${label}: rejected by Parcel (${result.statusCode})`;
    }

    // Unreachable: candidates is non-empty and every iteration returns or continues
    throw new Error(`No submission attempted for ${trackingNumber}`);
  }
}
