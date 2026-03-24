import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { FetchedEmail } from "../jmap/emailFetcher.js";
import config from "../utils/config.js";
import { logTimestamp } from "../utils/markdown.js";
import { isValidCarrierCode } from "./carriers/carrierMap.js";
import { extractDeliveries } from "./extraction/extractDeliveries.js";
import { filterTrackingCandidate } from "./filter/keywords.js";
import { submitDelivery } from "./parcel/parcelApi.js";
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
        return false;
      }
      return true;
    });

    if (newCandidates.length < candidates.length) {
      this.logger.info(
        `Skipped ${candidates.length - newCandidates.length} email(s) with known tracking numbers`,
      );
    }

    // Process each candidate
    for (const email of newCandidates) {
      const runLog = config.LOGS_PATH
        ? new LogFile(
            `${config.LOGS_PATH}/parcel-tracker/${logTimestamp()}.md`,
            "overwrite",
          )
        : undefined;
      try {
        await this.processEmail(email, runLog);
      } catch (error) {
        this.logger.error(
          `Failed to process email "${email.subject}"`,
          (error as Error).message,
        );
        // Continue with other emails
      }
    }
  }

  private async processEmail(
    email: {
      id: string;
      subject: string;
      from: string;
      textBody: string;
    },
    runLog?: LogFile,
  ): Promise<void> {
    this.logger.info(`Extracting from: "${email.subject}" (from: ${email.from})`);

    const deliveries = await extractDeliveries(
      { subject: email.subject, from: email.from, textBody: email.textBody },
      this.logger,
      runLog,
    );

    if (deliveries.length === 0) {
      this.logger.info(`No tracking numbers found in "${email.subject}"`);
      return;
    }

    this.logger.info(`Found ${deliveries.length} delivery(ies) in "${email.subject}"`);

    for (const delivery of deliveries) {
      await this.processDelivery(delivery, email.id);
    }
  }

  private async processDelivery(
    delivery: {
      tracking_number: string;
      carrier_code: string;
      description: string;
    },
    emailId: string,
  ): Promise<void> {
    // Dedup check
    if (hasSubmittedDelivery(delivery.tracking_number)) {
      this.logger.info(
        `Duplicate tracking number: ${delivery.tracking_number} (skipping)`,
      );
      return;
    }

    // Validate carrier code
    const valid = await isValidCarrierCode(delivery.carrier_code, this.logger);
    if (!valid) {
      this.logger.warn(
        `Invalid carrier code "${delivery.carrier_code}" for tracking ${delivery.tracking_number}, skipping`,
      );
      return;
    }

    // Submit to Parcel
    const result = await submitDelivery(
      {
        trackingNumber: delivery.tracking_number,
        carrierCode: delivery.carrier_code,
        description: delivery.description,
      },
      this.parcelApiKey,
      this.logger,
      this.rejectionLog,
    );

    if (result.status === "error") {
      this.logger.warn(
        `Failed to submit ${delivery.tracking_number} (${delivery.carrier_code}), will retry later`,
      );
      return;
    }

    if (result.status === "rejected") {
      this.logger.warn(
        `Parcel rejected ${delivery.tracking_number} (${delivery.carrier_code}) with ${result.statusCode}, recording to prevent retry`,
      );
    }

    // Record on success OR rejection to prevent retrying hopeless submissions
    recordSubmittedDelivery({
      trackingNumber: delivery.tracking_number,
      carrierCode: delivery.carrier_code,
      description: delivery.description,
      submittedAt: Date.now(),
      emailId,
    });
  }
}
