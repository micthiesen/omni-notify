import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import config from "../utils/config.js";
import { isValidCarrierCode } from "./carriers/carrierMap.js";
import { extractDeliveries } from "./extraction/extractDeliveries.js";
import { isTrackingCandidate } from "./filter/keywords.js";
import type { JmapContext } from "./jmap/client.js";
import { fetchNewEmails } from "./jmap/emailFetcher.js";
import { submitDelivery } from "./parcel/parcelApi.js";
import {
  getEmailState,
  hasSubmittedDelivery,
  recordSubmittedDelivery,
  saveEmailState,
} from "./persistence.js";

export class DeliveryPipeline {
  private logger: Logger;
  private ctx: JmapContext;
  private parcelApiKey: string;
  private processing = false;
  private runLog?: LogFile;
  private rejectionLog?: LogFile;

  constructor(ctx: JmapContext, parcelApiKey: string, logger: Logger) {
    this.ctx = ctx;
    this.parcelApiKey = parcelApiKey;
    this.logger = logger;

    if (config.LOGS_PATH) {
      const dir = `${config.LOGS_PATH}/parcel-tracker`;
      this.runLog = new LogFile(`${dir}/latest-run.log`, "overwrite");
      this.rejectionLog = new LogFile(`${dir}/rejections.log`, "append");
    }
  }

  async onEmailStateChange(): Promise<void> {
    if (this.processing) {
      this.logger.debug("Pipeline already processing, skipping");
      return;
    }

    this.processing = true;
    try {
      await this.processStateChange();
    } finally {
      this.processing = false;
    }
  }

  private async processStateChange(): Promise<void> {
    const sinceState = getEmailState();

    if (!sinceState) {
      // First run: save current Email state without processing
      this.logger.info("First run: fetching current JMAP state (skipping history)");
      const state = await this.fetchCurrentEmailState();
      if (state) {
        saveEmailState(state);
        this.logger.info(`Saved initial JMAP state: ${state}`);
      }
      return;
    }

    let emails: Awaited<ReturnType<typeof fetchNewEmails>>["emails"];
    let newState: string;
    try {
      const result = await fetchNewEmails(this.ctx, sinceState, this.logger);
      emails = result.emails;
      newState = result.newState;
    } catch (error) {
      const message = (error as Error).message ?? "";
      if (message.includes("cannotCalculateChanges")) {
        this.logger.warn("cannotCalculateChanges: resetting state");
        const state = await this.fetchCurrentEmailState();
        if (state) saveEmailState(state);
        return;
      }
      this.logger.error("Failed to fetch emails", message);
      return;
    }

    // Filter candidates
    const candidates = [];
    for (const email of emails) {
      const isCandidate = await isTrackingCandidate(
        { from: email.from, subject: email.subject, textBody: email.textBody },
        this.logger,
      );
      if (isCandidate) {
        candidates.push(email);
      } else {
        this.logger.debug(`Filtered out: "${email.subject}" from ${email.from}`);
      }
    }

    if (candidates.length > 0) {
      this.logger.info(
        `${candidates.length} tracking candidate(s) from ${emails.length} new email(s)`,
      );
    } else if (emails.length > 0) {
      this.logger.debug(`No tracking candidates in ${emails.length} new email(s)`);
    }

    // Process each candidate
    for (const email of candidates) {
      try {
        await this.processEmail(email);
      } catch (error) {
        this.logger.error(
          `Failed to process email "${email.subject}"`,
          (error as Error).message,
        );
        // Continue with other emails
      }
    }

    // Save state after processing (crash-safe: dedup prevents double-submit)
    saveEmailState(newState);
  }

  private async processEmail(email: {
    id: string;
    subject: string;
    from: string;
    textBody: string;
  }): Promise<void> {
    this.logger.info(`Extracting from: "${email.subject}" (from: ${email.from})`);

    const deliveries = await extractDeliveries(
      { subject: email.subject, from: email.from, textBody: email.textBody },
      this.logger,
      this.runLog,
    );

    if (deliveries.length === 0) {
      this.logger.debug(`No tracking numbers found in "${email.subject}"`);
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

  /** Get the current Email state via Email/get (compatible with Email/changes sinceState). */
  private async fetchCurrentEmailState(): Promise<string | undefined> {
    const [result] = await this.ctx.jam.request([
      "Email/get",
      { accountId: this.ctx.accountId, ids: [] },
    ]);
    return (result as Record<string, unknown>).state as string | undefined;
  }
}
